//! PM analytics: per-developer engagement + delivery leaderboard for an
//! organization. Phase 1 is read-only and derives every metric from existing
//! tables (auth_sessions, issues, issue_assignees, pull_requests, workspaces) —
//! no schema changes and no client telemetry. Admin-only.

use axum::{
    Json, Router,
    extract::{Extension, Path, Query, State},
    response::IntoResponse,
    routing::get,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    error::{ErrorResponse, db_error},
    organization_members::ensure_admin_access,
};
use crate::{AppState, auth::RequestContext};

pub(super) fn router() -> Router<AppState> {
    Router::new().route(
        "/organizations/{org_id}/insights",
        get(get_organization_insights),
    )
}

#[derive(Debug, Deserialize)]
struct InsightsQuery {
    /// One of `7d`, `30d`, `all`. Defaults to `30d`.
    window: Option<String>,
}

/// Resolve a window string to a lower-bound timestamp. `None` means "all time".
fn window_since(window: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    match window {
        "all" => None,
        "7d" => Some(now - Duration::days(7)),
        // Default to 30d for "30d" and any unrecognized value.
        _ => Some(now - Duration::days(30)),
    }
}

/// Ranking weights. Kept transparent so the leaderboard is explainable; the
/// frontend can still sort by any individual column.
const W_MR_MERGED: i64 = 5;
const W_MR_OPENED: i64 = 2;
const W_ISSUE_COMPLETED: i64 = 3;
const W_ISSUE_ASSIGNED: i64 = 1;

#[derive(Debug, sqlx::FromRow)]
struct InsightsRow {
    user_id: Uuid,
    email: String,
    first_name: Option<String>,
    last_name: Option<String>,
    username: Option<String>,
    role: String,
    last_active_at: Option<DateTime<Utc>>,
    issues_assigned: i64,
    issues_completed: i64,
    mrs_opened: i64,
    mrs_merged: i64,
}

// NOTE: Phase 1 returns plain JSON (no ts-rs `TS` derive) so the frontend type
// is hand-written in web-core. To switch to generated types later, add `TS` to
// these derives, register them in `bin/generate_types.rs`, and run
// `pnpm run remote:generate-types`.
#[derive(Debug, Serialize)]
pub struct DeveloperInsights {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub username: Option<String>,
    pub role: String,
    pub last_active_at: Option<DateTime<Utc>>,
    pub issues_assigned: i64,
    pub issues_completed: i64,
    pub mrs_opened: i64,
    pub mrs_merged: i64,
    pub score: i64,
}

/// One weekly bucket of completed-issue counts (delivery throughput).
#[derive(Debug, Serialize)]
pub struct ThroughputBucket {
    pub week_start: DateTime<Utc>,
    pub count: i64,
}

/// Org-level delivery metrics over the selected window. Cycle time = time from
/// issue creation to completion (Done); throughput = issues completed per week.
#[derive(Debug, Serialize)]
pub struct DeliverySummary {
    pub completed_count: i64,
    pub avg_cycle_time_hours: Option<f64>,
    pub median_cycle_time_hours: Option<f64>,
    pub throughput: Vec<ThroughputBucket>,
}

#[derive(Debug, sqlx::FromRow)]
struct CycleRow {
    completed_count: i64,
    avg_hours: Option<f64>,
    median_hours: Option<f64>,
}

#[derive(Debug, sqlx::FromRow)]
struct BucketRow {
    week_start: DateTime<Utc>,
    count: i64,
}

#[derive(Debug, Serialize)]
pub struct InsightsResponse {
    pub organization_id: Uuid,
    pub window: String,
    pub since: Option<DateTime<Utc>>,
    pub generated_at: DateTime<Utc>,
    pub developers: Vec<DeveloperInsights>,
    pub summary: DeliverySummary,
}

fn display_name(row: &InsightsRow) -> String {
    let full = [row.first_name.as_deref(), row.last_name.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    let full = full.trim();
    if !full.is_empty() {
        full.to_string()
    } else if let Some(username) = row.username.as_deref().filter(|u| !u.is_empty()) {
        username.to_string()
    } else {
        row.email.clone()
    }
}

async fn get_organization_insights(
    State(state): State<AppState>,
    Extension(ctx): Extension<RequestContext>,
    Path(org_id): Path<Uuid>,
    Query(query): Query<InsightsQuery>,
) -> Result<impl IntoResponse, ErrorResponse> {
    ensure_admin_access(&state.pool, org_id, ctx.user.id).await?;

    let now = Utc::now();
    let window = query.window.as_deref().unwrap_or("30d").to_string();
    let since = window_since(&window, now);

    let rows = sqlx::query_as::<_, InsightsRow>(
        r#"
        WITH members AS (
            SELECT m.user_id, u.email, u.first_name, u.last_name, u.username,
                   m.role::text AS role, m.last_seen_at
            FROM organization_member_metadata m
            JOIN users u ON u.id = m.user_id
            WHERE m.organization_id = $1
        ),
        org_projects AS (
            SELECT id FROM projects WHERE organization_id = $1
        ),
        last_sessions AS (
            SELECT s.user_id, MAX(s.last_used_at) AS last_session_at
            FROM auth_sessions s
            GROUP BY s.user_id
        ),
        issues_assigned AS (
            SELECT ia.user_id, COUNT(DISTINCT i.id)::bigint AS issues_assigned
            FROM issues i
            JOIN issue_assignees ia ON ia.issue_id = i.id
            WHERE i.project_id IN (SELECT id FROM org_projects)
              AND ($2::timestamptz IS NULL OR ia.assigned_at >= $2)
            GROUP BY ia.user_id
        ),
        issues_completed AS (
            -- completed_at is kept in sync with the Done status by a DB trigger
            -- (migration 20260625010000), so it gives accurate time windows.
            SELECT ia.user_id, COUNT(DISTINCT i.id)::bigint AS issues_completed
            FROM issues i
            JOIN issue_assignees ia ON ia.issue_id = i.id
            WHERE i.project_id IN (SELECT id FROM org_projects)
              AND i.completed_at IS NOT NULL
              AND ($2::timestamptz IS NULL OR i.completed_at >= $2)
            GROUP BY ia.user_id
        ),
        mrs_opened AS (
            SELECT w.owner_user_id AS user_id, COUNT(*)::bigint AS mrs_opened
            FROM pull_requests pr
            JOIN workspaces w ON w.id = pr.workspace_id
            WHERE w.project_id IN (SELECT id FROM org_projects)
              AND ($2::timestamptz IS NULL OR pr.created_at >= $2)
            GROUP BY w.owner_user_id
        ),
        mrs_merged AS (
            SELECT w.owner_user_id AS user_id, COUNT(*)::bigint AS mrs_merged
            FROM pull_requests pr
            JOIN workspaces w ON w.id = pr.workspace_id
            WHERE w.project_id IN (SELECT id FROM org_projects)
              AND pr.status = 'merged'
              AND ($2::timestamptz IS NULL OR pr.merged_at >= $2)
            GROUP BY w.owner_user_id
        )
        SELECT
            m.user_id,
            m.email,
            m.first_name,
            m.last_name,
            m.username,
            m.role,
            GREATEST(m.last_seen_at, ls.last_session_at) AS last_active_at,
            COALESCE(ia.issues_assigned, 0)   AS issues_assigned,
            COALESCE(icp.issues_completed, 0) AS issues_completed,
            COALESCE(mo.mrs_opened, 0)        AS mrs_opened,
            COALESCE(mm.mrs_merged, 0)        AS mrs_merged
        FROM members m
        LEFT JOIN last_sessions ls    ON ls.user_id  = m.user_id
        LEFT JOIN issues_assigned ia  ON ia.user_id = m.user_id
        LEFT JOIN issues_completed icp ON icp.user_id = m.user_id
        LEFT JOIN mrs_opened mo       ON mo.user_id = m.user_id
        LEFT JOIN mrs_merged mm       ON mm.user_id = m.user_id
        "#,
    )
    .bind(org_id)
    .bind(since)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| db_error(e, "Failed to load insights"))?;

    let mut developers: Vec<DeveloperInsights> = rows
        .into_iter()
        .map(|row| {
            let score = row.mrs_merged * W_MR_MERGED
                + row.mrs_opened * W_MR_OPENED
                + row.issues_completed * W_ISSUE_COMPLETED
                + row.issues_assigned * W_ISSUE_ASSIGNED;
            DeveloperInsights {
                display_name: display_name(&row),
                user_id: row.user_id,
                email: row.email,
                username: row.username,
                role: row.role,
                last_active_at: row.last_active_at,
                issues_assigned: row.issues_assigned,
                issues_completed: row.issues_completed,
                mrs_opened: row.mrs_opened,
                mrs_merged: row.mrs_merged,
                score,
            }
        })
        .collect();

    // Default ranking: score desc, then merged MRs, then most recently active.
    developers.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then(b.mrs_merged.cmp(&a.mrs_merged))
            .then(b.last_active_at.cmp(&a.last_active_at))
    });

    // Delivery summary: cycle time (created -> completed) over the window.
    let cycle = sqlx::query_as::<_, CycleRow>(
        r#"
        SELECT
            COUNT(*)::bigint AS completed_count,
            AVG(EXTRACT(EPOCH FROM (i.completed_at - i.created_at))::double precision / 3600.0) AS avg_hours,
            PERCENTILE_CONT(0.5) WITHIN GROUP (
                ORDER BY EXTRACT(EPOCH FROM (i.completed_at - i.created_at))::double precision / 3600.0
            ) AS median_hours
        FROM issues i
        WHERE i.project_id IN (SELECT id FROM projects WHERE organization_id = $1)
          AND i.completed_at IS NOT NULL
          AND ($2::timestamptz IS NULL OR i.completed_at >= $2)
        "#,
    )
    .bind(org_id)
    .bind(since)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| db_error(e, "Failed to load cycle time"))?;

    // Throughput: completed issues per ISO week.
    let buckets = sqlx::query_as::<_, BucketRow>(
        r#"
        SELECT date_trunc('week', i.completed_at) AS week_start,
               COUNT(*)::bigint AS count
        FROM issues i
        WHERE i.project_id IN (SELECT id FROM projects WHERE organization_id = $1)
          AND i.completed_at IS NOT NULL
          AND ($2::timestamptz IS NULL OR i.completed_at >= $2)
        GROUP BY 1
        ORDER BY 1
        "#,
    )
    .bind(org_id)
    .bind(since)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| db_error(e, "Failed to load throughput"))?;

    let summary = DeliverySummary {
        completed_count: cycle.completed_count,
        avg_cycle_time_hours: cycle.avg_hours,
        median_cycle_time_hours: cycle.median_hours,
        throughput: buckets
            .into_iter()
            .map(|b| ThroughputBucket {
                week_start: b.week_start,
                count: b.count,
            })
            .collect(),
    };

    Ok(Json(InsightsResponse {
        organization_id: org_id,
        window,
        since,
        generated_at: now,
        developers,
        summary,
    }))
}
