use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

/// A user assigned to a project. Drives the launcher's "Personal" tab.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ProjectMember {
    pub id: Uuid,
    pub project_id: Uuid,
    pub user_id: Uuid,
    pub assigned_at: DateTime<Utc>,
}

/// Replace the full set of members assigned to a project (idempotent).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SetProjectMembersRequest {
    pub user_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ListProjectMembersResponse {
    pub project_members: Vec<ProjectMember>,
    pub txid: i64,
}
