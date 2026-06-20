use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Type;
use ts_rs::TS;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type, TS)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[sqlx(type_name = "member_role", rename_all = "lowercase")]
#[ts(use_ts_enum)]
#[ts(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MemberRole {
    Admin,
    Member,
}

/// Organization member as stored in the database / streamed via Electric.
/// This is the full row type with organization_id for shapes.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct OrganizationMember {
    pub organization_id: Uuid,
    pub user_id: Uuid,
    pub role: MemberRole,
    pub joined_at: DateTime<Utc>,
    pub last_seen_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ProvisionMemberRequest {
    pub email: String,
    pub role: MemberRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_name: Option<String>,
    /// Optional handle. When omitted, the server derives one from the email prefix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// When true, a generated temporary password is returned in the response (for
    /// admin tooling that prefers in-band delivery to email). The user is still
    /// required to change it on first login.
    #[serde(default)]
    pub return_password: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ProvisionMemberResponse {
    pub user_id: Uuid,
    pub email: String,
    pub role: MemberRole,
    pub organization_id: Uuid,
    /// Only populated when the request asked for in-band password return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temporary_password: Option<String>,
}
