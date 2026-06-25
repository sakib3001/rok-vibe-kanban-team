//! Admin approval workflow for completing issues. When a project has
//! `requires_done_approval`, moving an issue to Done parks it as `pending`
//! (the DB trigger keeps `completed_at` NULL). An admin then approves (→ truly
//! complete) or rejects (→ back to In Progress with a note).
//!
//! Approval columns live on the issues/projects tables but are intentionally
//! NOT part of the synced Issue/Project structs — they're exposed only here, so
//! the existing column-explicit queries are untouched.

use axum::{
    Json, Router,
    extract::{Extension, Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    error::{ErrorResponse, db_error},
    organization_members::ensure_admin_access,
};
use crate::{
    AppState,
    auth::RequestContext,
    db::{begin_tx, get_txid, projects::ProjectRepository},
};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/organizations/{org_id}/pending-approvals",
            get(list_pending_approvals),
        )
        .route(
            "/organizations/{org_id}/project-approval-settings",
            get(list_project_approval_settings),
        )
        .route(
            "/projects/{project_id}/approval-setting",
            put(set_project_approval_setting),
        )
        .route("/issues/{issue_id}/approve", post(approve_issue))
        .route("/issues/{issue_id}/reject", post(reject_issue))
}

/// (project_id, organization_id) for an issue, or 404.
async fn issue_project_org(
    state: &AppState,
    issue_id: Uuid,
) -> Result<(Uuid, Uuid), ErrorResponse> {
    let row: Option<(Uuid, Uuid)> = sqlx::query_as(
        r#"
        SELECT i.project_id, p.organization_id
        FROM issues i JOIN projects p ON p.id = i.project_id
        WHERE i.id = $1
        "#,
    )
    .bind(issue_id)
    .fetch_optional(state.pool())
    .await
    .map_err(|e| db_error(e, "failed to load issue"))?;
    row.ok_or_else(|| ErrorResponse::new(StatusCode::NOT_FOUND, "issue not found"))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct PendingApproval {
    issue_id: Uuid,
    simple_id: String,
    title: String,
    project_id: Uuid,
    project_name: String,
    assignees: String,
    submitted_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
struct PendingApprovalsResponse {
    pending: Vec<PendingApproval>,
}

async fn list_pending_approvals(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(org_id): Path<Uuid>,
) -> Result<impl IntoResponse, ErrorResponse> {
    ensure_admin_access(state.pool(), org_id, ctx.user.id).await?;

    let pending = sqlx::query_as::<_, PendingApproval>(
        r#"
        SELECT
            i.id            AS issue_id,
            i.simple_id     AS simple_id,
            i.title         AS title,
            i.project_id    AS project_id,
            p.name          AS project_name,
            COALESCE(string_agg(
                DISTINCT COALESCE(
                    NULLIF(TRIM(CONCAT(u.first_name, ' ', u.last_name)), ''),
                    u.username, u.email
                ), ', '
            ), '')          AS assignees,
            i.updated_at    AS submitted_at
        FROM issues i
        JOIN projects p ON p.id = i.project_id
        LEFT JOIN issue_assignees ia ON ia.issue_id = i.id
        LEFT JOIN users u ON u.id = ia.user_id
        WHERE p.organization_id = $1 AND i.approval_state = 'pending'
        GROUP BY i.id, i.simple_id, i.title, i.project_id, p.name, i.updated_at
        ORDER BY i.updated_at ASC
        "#,
    )
    .bind(org_id)
    .fetch_all(state.pool())
    .await
    .map_err(|e| db_error(e, "failed to load pending approvals"))?;

    Ok(Json(PendingApprovalsResponse { pending }))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct ProjectApprovalSetting {
    project_id: Uuid,
    requires_done_approval: bool,
}

#[derive(Debug, Serialize)]
struct ProjectApprovalSettingsResponse {
    settings: Vec<ProjectApprovalSetting>,
}

async fn list_project_approval_settings(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(org_id): Path<Uuid>,
) -> Result<impl IntoResponse, ErrorResponse> {
    ensure_admin_access(state.pool(), org_id, ctx.user.id).await?;

    let settings = sqlx::query_as::<_, ProjectApprovalSetting>(
        r#"
        SELECT id AS project_id, requires_done_approval
        FROM projects WHERE organization_id = $1
        "#,
    )
    .bind(org_id)
    .fetch_all(state.pool())
    .await
    .map_err(|e| db_error(e, "failed to load approval settings"))?;

    Ok(Json(ProjectApprovalSettingsResponse { settings }))
}

#[derive(Debug, Deserialize)]
struct SetApprovalSettingRequest {
    requires_done_approval: bool,
}

#[derive(Debug, Serialize)]
struct OkResponse {
    ok: bool,
    txid: i64,
}

async fn set_project_approval_setting(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(project_id): Path<Uuid>,
    Json(payload): Json<SetApprovalSettingRequest>,
) -> Result<impl IntoResponse, ErrorResponse> {
    let org_id = ProjectRepository::organization_id(state.pool(), project_id)
        .await
        .map_err(|e| db_error(e, "failed to load project"))?
        .ok_or_else(|| ErrorResponse::new(StatusCode::NOT_FOUND, "project not found"))?;
    ensure_admin_access(state.pool(), org_id, ctx.user.id).await?;

    let mut tx = begin_tx(state.pool())
        .await
        .map_err(|e| db_error(e, "failed to begin transaction"))?;
    sqlx::query("UPDATE projects SET requires_done_approval = $2 WHERE id = $1")
        .bind(project_id)
        .bind(payload.requires_done_approval)
        .execute(&mut *tx)
        .await
        .map_err(|e| db_error(e, "failed to update setting"))?;
    let txid = get_txid(&mut *tx)
        .await
        .map_err(|e| db_error(e, "failed to get txid"))?;
    tx.commit()
        .await
        .map_err(|e| db_error(e, "failed to commit"))?;

    Ok(Json(OkResponse { ok: true, txid }))
}

async fn approve_issue(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(issue_id): Path<Uuid>,
) -> Result<impl IntoResponse, ErrorResponse> {
    let (_project_id, org_id) = issue_project_org(&state, issue_id).await?;
    ensure_admin_access(state.pool(), org_id, ctx.user.id).await?;

    let mut tx = begin_tx(state.pool())
        .await
        .map_err(|e| db_error(e, "failed to begin transaction"))?;
    // Trigger stamps completed_at because status is Done + approval_state approved.
    let result = sqlx::query(
        r#"
        UPDATE issues
        SET approval_state = 'approved', approved_by = $2, approved_at = NOW(),
            approval_note = NULL
        WHERE id = $1 AND approval_state = 'pending'
        "#,
    )
    .bind(issue_id)
    .bind(ctx.user.id)
    .execute(&mut *tx)
    .await
    .map_err(|e| db_error(e, "failed to approve issue"))?;

    if result.rows_affected() == 0 {
        return Err(ErrorResponse::new(
            StatusCode::CONFLICT,
            "issue is not pending approval",
        ));
    }

    let txid = get_txid(&mut *tx)
        .await
        .map_err(|e| db_error(e, "failed to get txid"))?;
    tx.commit()
        .await
        .map_err(|e| db_error(e, "failed to commit"))?;

    Ok(Json(OkResponse { ok: true, txid }))
}

#[derive(Debug, Deserialize)]
struct RejectRequest {
    #[serde(default)]
    note: Option<String>,
}

async fn reject_issue(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(issue_id): Path<Uuid>,
    Json(payload): Json<RejectRequest>,
) -> Result<impl IntoResponse, ErrorResponse> {
    let (project_id, org_id) = issue_project_org(&state, issue_id).await?;
    ensure_admin_access(state.pool(), org_id, ctx.user.id).await?;

    // Target status: the project's "In Progress" (fallback "To do").
    let target: Option<Uuid> = sqlx::query_scalar(
        r#"
        SELECT id FROM project_statuses
        WHERE project_id = $1 AND lower(name) IN ('in progress', 'to do')
        ORDER BY (lower(name) = 'in progress') DESC, sort_order ASC
        LIMIT 1
        "#,
    )
    .bind(project_id)
    .fetch_optional(state.pool())
    .await
    .map_err(|e| db_error(e, "failed to resolve target status"))?;

    let target = target.ok_or_else(|| {
        ErrorResponse::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "project has no In Progress / To do status to return the issue to",
        )
    })?;

    let mut tx = begin_tx(state.pool())
        .await
        .map_err(|e| db_error(e, "failed to begin transaction"))?;
    // Moving off Done makes the trigger clear completed_at and reset approval
    // state; the note is preserved for the developer.
    let result = sqlx::query(
        r#"
        UPDATE issues
        SET status_id = $2, approval_note = $3
        WHERE id = $1 AND approval_state = 'pending'
        "#,
    )
    .bind(issue_id)
    .bind(target)
    .bind(payload.note)
    .execute(&mut *tx)
    .await
    .map_err(|e| db_error(e, "failed to reject issue"))?;

    if result.rows_affected() == 0 {
        return Err(ErrorResponse::new(
            StatusCode::CONFLICT,
            "issue is not pending approval",
        ));
    }

    let txid = get_txid(&mut *tx)
        .await
        .map_err(|e| db_error(e, "failed to get txid"))?;
    tx.commit()
        .await
        .map_err(|e| db_error(e, "failed to commit"))?;

    Ok(Json(OkResponse { ok: true, txid }))
}
