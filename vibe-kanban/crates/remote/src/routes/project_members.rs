//! Admin-managed project assignment. An org admin assigns whole projects to
//! specific members; the launcher's "Personal" tab reads the resulting
//! `project_members` rows (via Electric sync) to filter the project list.
//!
//! Mutations are admin-only; listing requires org membership. Assignment is a
//! visibility filter, not an access boundary (see the migration for rationale).

use api_types::{ListProjectMembersResponse, SetProjectMembersRequest};
use axum::{
    Json, Router,
    extract::{Extension, Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get},
};
use uuid::Uuid;

use super::{
    error::{ErrorResponse, db_error},
    organization_members::{ensure_admin_access, ensure_member_access},
};
use crate::{
    AppState,
    auth::RequestContext,
    db::{project_members::ProjectMemberRepository, projects::ProjectRepository},
};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project_id}/members",
            get(list_project_members).put(set_project_members),
        )
        .route(
            "/projects/{project_id}/members/{user_id}",
            delete(unassign_project_member),
        )
}

/// Resolve the organization that owns a project, or 404.
async fn project_org_id(state: &AppState, project_id: Uuid) -> Result<Uuid, ErrorResponse> {
    ProjectRepository::organization_id(state.pool(), project_id)
        .await
        .map_err(|e| db_error(e, "failed to load project"))?
        .ok_or_else(|| ErrorResponse::new(StatusCode::NOT_FOUND, "project not found"))
}

async fn list_project_members(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(project_id): Path<Uuid>,
) -> Result<impl IntoResponse, ErrorResponse> {
    let org_id = project_org_id(&state, project_id).await?;
    ensure_member_access(state.pool(), org_id, ctx.user.id).await?;

    let project_members = ProjectMemberRepository::list_by_project(state.pool(), project_id)
        .await
        .map_err(|e| db_error(e, "failed to list project members"))?;

    Ok(Json(ListProjectMembersResponse {
        project_members,
        txid: 0,
    }))
}

async fn set_project_members(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(project_id): Path<Uuid>,
    Json(payload): Json<SetProjectMembersRequest>,
) -> Result<impl IntoResponse, ErrorResponse> {
    let org_id = project_org_id(&state, project_id).await?;
    ensure_admin_access(state.pool(), org_id, ctx.user.id).await?;

    // Reject any user that isn't a member of the project's organization.
    let valid = ProjectMemberRepository::filter_org_members(state.pool(), org_id, &payload.user_ids)
        .await
        .map_err(|e| db_error(e, "failed to validate members"))?;
    if valid.len() != payload.user_ids.len() {
        return Err(ErrorResponse::new(
            StatusCode::BAD_REQUEST,
            "all assignees must be members of the project's organization",
        ));
    }

    let txid =
        ProjectMemberRepository::replace_set(state.pool(), project_id, &payload.user_ids, ctx.user.id)
            .await
            .map_err(|e| db_error(e, "failed to update project members"))?;

    let project_members = ProjectMemberRepository::list_by_project(state.pool(), project_id)
        .await
        .map_err(|e| db_error(e, "failed to list project members"))?;

    Ok(Json(ListProjectMembersResponse {
        project_members,
        txid,
    }))
}

async fn unassign_project_member(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path((project_id, user_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, ErrorResponse> {
    let org_id = project_org_id(&state, project_id).await?;
    ensure_admin_access(state.pool(), org_id, ctx.user.id).await?;

    let txid = ProjectMemberRepository::unassign(state.pool(), project_id, user_id)
        .await
        .map_err(|e| db_error(e, "failed to unassign project member"))?;

    let project_members = ProjectMemberRepository::list_by_project(state.pool(), project_id)
        .await
        .map_err(|e| db_error(e, "failed to list project members"))?;

    Ok(Json(ListProjectMembersResponse {
        project_members,
        txid,
    }))
}
