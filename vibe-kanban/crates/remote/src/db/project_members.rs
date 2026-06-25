//! Project assignment storage. Uses sqlx's runtime-checked query API (not the
//! compile-time `query!` macros) so the build doesn't depend on a regenerated
//! `.sqlx` offline cache — same approach as `routes/insights.rs`.

use api_types::{Project, ProjectMember};
use chrono::{DateTime, Utc};
use sqlx::{Executor, PgPool, Postgres};
use uuid::Uuid;

use super::get_txid;

#[derive(Debug, sqlx::FromRow)]
struct ProjectMemberRow {
    id: Uuid,
    project_id: Uuid,
    user_id: Uuid,
    assigned_at: DateTime<Utc>,
}

impl From<ProjectMemberRow> for ProjectMember {
    fn from(r: ProjectMemberRow) -> Self {
        ProjectMember {
            id: r.id,
            project_id: r.project_id,
            user_id: r.user_id,
            assigned_at: r.assigned_at,
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct ProjectRow {
    id: Uuid,
    organization_id: Uuid,
    name: String,
    color: String,
    sort_order: i32,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<ProjectRow> for Project {
    fn from(r: ProjectRow) -> Self {
        Project {
            id: r.id,
            organization_id: r.organization_id,
            name: r.name,
            color: r.color,
            sort_order: r.sort_order,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

pub struct ProjectMemberRepository;

impl ProjectMemberRepository {
    /// List the members assigned to a project.
    pub async fn list_by_project<'e, E>(
        executor: E,
        project_id: Uuid,
    ) -> Result<Vec<ProjectMember>, sqlx::Error>
    where
        E: Executor<'e, Database = Postgres>,
    {
        let rows = sqlx::query_as::<_, ProjectMemberRow>(
            r#"
            SELECT id, project_id, user_id, assigned_at
            FROM project_members
            WHERE project_id = $1
            ORDER BY assigned_at ASC
            "#,
        )
        .bind(project_id)
        .fetch_all(executor)
        .await?;

        Ok(rows.into_iter().map(Into::into).collect())
    }

    /// The projects in an org that the given user is assigned to (Personal view).
    pub async fn list_assigned_projects<'e, E>(
        executor: E,
        organization_id: Uuid,
        user_id: Uuid,
    ) -> Result<Vec<Project>, sqlx::Error>
    where
        E: Executor<'e, Database = Postgres>,
    {
        let rows = sqlx::query_as::<_, ProjectRow>(
            r#"
            SELECT p.id, p.organization_id, p.name, p.color, p.sort_order,
                   p.created_at, p.updated_at
            FROM projects p
            JOIN project_members pm ON pm.project_id = p.id
            WHERE p.organization_id = $1 AND pm.user_id = $2
            ORDER BY p.sort_order ASC, p.created_at DESC
            "#,
        )
        .bind(organization_id)
        .bind(user_id)
        .fetch_all(executor)
        .await?;

        Ok(rows.into_iter().map(Into::into).collect())
    }

    /// Of `user_ids`, return those that are members of `organization_id`.
    /// Used to reject assigning a project to someone outside the org.
    pub async fn filter_org_members(
        pool: &PgPool,
        organization_id: Uuid,
        user_ids: &[Uuid],
    ) -> Result<Vec<Uuid>, sqlx::Error> {
        let rows: Vec<(Uuid,)> = sqlx::query_as(
            r#"
            SELECT user_id
            FROM organization_member_metadata
            WHERE organization_id = $1 AND user_id = ANY($2)
            "#,
        )
        .bind(organization_id)
        .bind(user_ids)
        .fetch_all(pool)
        .await?;

        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    /// Replace the full assignee set for a project (idempotent). Returns the txid.
    pub async fn replace_set(
        pool: &PgPool,
        project_id: Uuid,
        user_ids: &[Uuid],
        assigned_by: Uuid,
    ) -> Result<i64, sqlx::Error> {
        let mut tx = super::begin_tx(pool).await?;

        sqlx::query("DELETE FROM project_members WHERE project_id = $1")
            .bind(project_id)
            .execute(&mut *tx)
            .await?;

        for user_id in user_ids {
            sqlx::query(
                r#"
                INSERT INTO project_members (project_id, user_id, assigned_by)
                VALUES ($1, $2, $3)
                ON CONFLICT (project_id, user_id) DO NOTHING
                "#,
            )
            .bind(project_id)
            .bind(user_id)
            .bind(assigned_by)
            .execute(&mut *tx)
            .await?;
        }

        let txid = get_txid(&mut *tx).await?;
        tx.commit().await?;
        Ok(txid)
    }

    /// Unassign a single user from a project. Returns the txid.
    pub async fn unassign(
        pool: &PgPool,
        project_id: Uuid,
        user_id: Uuid,
    ) -> Result<i64, sqlx::Error> {
        let mut tx = super::begin_tx(pool).await?;

        sqlx::query("DELETE FROM project_members WHERE project_id = $1 AND user_id = $2")
            .bind(project_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;

        let txid = get_txid(&mut *tx).await?;
        tx.commit().await?;
        Ok(txid)
    }
}
