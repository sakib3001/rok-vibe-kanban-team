use api_types::{
    ChangePasswordRequest, CredentialLoginRequest, CredentialLoginResponse, MemberRole,
    PasswordResetCompleteRequest, ProvisionMemberRequest, ProvisionMemberResponse,
};
use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration, Utc};
use rand::{Rng, RngCore, distr::Alphanumeric};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    AppState,
    db::{
        auth::AuthSessionRepository,
        identity_errors::IdentityError,
        organization_members::{add_member, assert_admin, is_member},
        organizations::{OrganizationRepository, is_personal_org},
        password_reset_tokens::PasswordResetTokenRepository,
        user_passwords::UserPasswordRepository,
        users::{UpsertUser, UserRepository},
    },
};

pub(super) const CREDENTIAL_AUTH_PROVIDER: &str = "credential";
const RESET_TOKEN_TTL_HOURS: i64 = 1;
const TEMPORARY_PASSWORD_LEN: usize = 16;
const MIN_PASSWORD_LEN: usize = 8;
const RESET_TOKEN_BYTES: usize = 32;

#[derive(Debug, thiserror::Error)]
pub(crate) enum CredentialAuthError {
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("user not found")]
    UserNotFound,
    #[error("password does not meet complexity requirements")]
    WeakPassword,
    #[error("invalid or expired reset token")]
    InvalidResetToken,
    #[error("permission denied")]
    PermissionDenied,
    #[error("invitations cannot be sent for personal organizations")]
    PersonalOrgInvite,
    #[error("user is already a member of this organization")]
    AlreadyMember,
    #[error("invalid email")]
    InvalidEmail,
    #[error("internal error")]
    Internal,
}

impl IntoResponse for CredentialAuthError {
    fn into_response(self) -> Response {
        let (status, code) = match self {
            CredentialAuthError::InvalidCredentials => {
                (StatusCode::UNAUTHORIZED, "invalid_credentials")
            }
            CredentialAuthError::UserNotFound => (StatusCode::NOT_FOUND, "user_not_found"),
            CredentialAuthError::WeakPassword => (StatusCode::BAD_REQUEST, "weak_password"),
            CredentialAuthError::InvalidResetToken => {
                (StatusCode::BAD_REQUEST, "invalid_reset_token")
            }
            CredentialAuthError::PermissionDenied => (StatusCode::FORBIDDEN, "permission_denied"),
            CredentialAuthError::PersonalOrgInvite => {
                (StatusCode::BAD_REQUEST, "personal_org_invite")
            }
            CredentialAuthError::AlreadyMember => (StatusCode::CONFLICT, "already_member"),
            CredentialAuthError::InvalidEmail => (StatusCode::BAD_REQUEST, "invalid_email"),
            CredentialAuthError::Internal => {
                (StatusCode::INTERNAL_SERVER_ERROR, "internal_error")
            }
        };
        (
            status,
            Json(serde_json::json!({
                "error": code,
                "message": self.to_string(),
            })),
        )
            .into_response()
    }
}

fn map_identity(err: IdentityError) -> CredentialAuthError {
    match err {
        IdentityError::NotFound => CredentialAuthError::UserNotFound,
        IdentityError::PermissionDenied => CredentialAuthError::PermissionDenied,
        IdentityError::Database(e) => {
            tracing::error!(?e, "credential auth db error");
            CredentialAuthError::Internal
        }
        other => {
            tracing::error!(?other, "credential auth identity error");
            CredentialAuthError::Internal
        }
    }
}

pub(crate) fn hash_password(plaintext: &str) -> Result<String, CredentialAuthError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(plaintext.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| {
            tracing::error!(?e, "argon2 hash failed");
            CredentialAuthError::Internal
        })
}

fn verify_password(plaintext: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(plaintext.as_bytes(), &parsed)
        .is_ok()
}

fn validate_password(password: &str) -> Result<(), CredentialAuthError> {
    if password.len() < MIN_PASSWORD_LEN {
        return Err(CredentialAuthError::WeakPassword);
    }
    Ok(())
}

fn validate_email(email: &str) -> Result<String, CredentialAuthError> {
    let normalized = email.trim().to_ascii_lowercase();
    if normalized.is_empty() || !normalized.contains('@') {
        return Err(CredentialAuthError::InvalidEmail);
    }
    Ok(normalized)
}

fn email_local_part(email: &str) -> &str {
    email.split('@').next().unwrap_or("")
}

/// Derive a presentable first name from the email local part — capitalised
/// first segment before a dot/underscore/dash/plus. e.g. `john.doe+work@ex.com`
/// -> `John`. Falls back to the whole local part if no separator.
fn derive_first_name_from_email(email: &str) -> String {
    let local = email_local_part(email);
    let first = local
        .split(['.', '_', '-', '+'])
        .find(|s| !s.is_empty())
        .unwrap_or(local);
    let mut chars = first.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Derive a username/handle from the email local part. Lowercase, with `+tag`
/// suffix stripped. e.g. `John.Doe+work@ex.com` -> `john.doe`.
fn derive_username_from_email(email: &str) -> String {
    let local = email_local_part(email);
    let base = local.split('+').next().unwrap_or(local);
    base.to_ascii_lowercase()
}

fn generate_temporary_password() -> String {
    rand::rng()
        .sample_iter(&Alphanumeric)
        .take(TEMPORARY_PASSWORD_LEN)
        .map(char::from)
        .collect()
}

fn generate_reset_token() -> (String, String) {
    let mut bytes = [0u8; RESET_TOKEN_BYTES];
    rand::rng().fill_bytes(&mut bytes);
    let raw = URL_SAFE_NO_PAD.encode(bytes);
    let hash = hex::encode(Sha256::digest(raw.as_bytes()));
    (raw, hash)
}

pub(crate) async fn login(
    state: &AppState,
    payload: &CredentialLoginRequest,
) -> Result<CredentialLoginResponse, CredentialAuthError> {
    let email = validate_email(&payload.email)?;

    let user_repo = UserRepository::new(state.pool());
    let pw_repo = UserPasswordRepository::new(state.pool());
    let org_repo = OrganizationRepository::new(state.pool());
    let session_repo = AuthSessionRepository::new(state.pool());

    let user = user_repo
        .fetch_user_by_email(&email)
        .await
        .map_err(map_identity)?
        .ok_or(CredentialAuthError::InvalidCredentials)?;

    let stored = pw_repo
        .fetch_by_user(user.id)
        .await
        .map_err(map_identity)?
        .ok_or(CredentialAuthError::InvalidCredentials)?;

    if !verify_password(&payload.password, &stored.hash) {
        return Err(CredentialAuthError::InvalidCredentials);
    }

    org_repo
        .ensure_personal_org_and_admin_membership(user.id, user.username.as_deref())
        .await
        .map_err(map_identity)?;

    let session = session_repo.create(user.id, None).await.map_err(|e| {
        tracing::error!(?e, "credential login: session create failed");
        CredentialAuthError::Internal
    })?;

    let tokens = state
        .jwt()
        .generate_tokens(&session, &user, CREDENTIAL_AUTH_PROVIDER)
        .map_err(|e| {
            tracing::error!(?e, "credential login: token generation failed");
            CredentialAuthError::Internal
        })?;

    session_repo
        .set_current_refresh_token(session.id, tokens.refresh_token_id)
        .await
        .map_err(|e| {
            tracing::error!(?e, "credential login: persist refresh token failed");
            CredentialAuthError::Internal
        })?;

    if let Some(analytics) = state.analytics() {
        analytics.track(
            user.id,
            "$identify",
            serde_json::json!({ "email": user.email }),
        );
    }

    Ok(CredentialLoginResponse {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        must_change_password: stored.must_change,
    })
}

pub(crate) async fn change_password(
    state: &AppState,
    user_id: Uuid,
    payload: &ChangePasswordRequest,
) -> Result<(), CredentialAuthError> {
    validate_password(&payload.new_password)?;

    let pw_repo = UserPasswordRepository::new(state.pool());
    let stored = pw_repo
        .fetch_by_user(user_id)
        .await
        .map_err(map_identity)?
        .ok_or(CredentialAuthError::InvalidCredentials)?;

    if !verify_password(&payload.current_password, &stored.hash) {
        return Err(CredentialAuthError::InvalidCredentials);
    }

    let new_hash = hash_password(&payload.new_password)?;
    pw_repo
        .upsert(user_id, &new_hash, false)
        .await
        .map_err(map_identity)?;

    // Invalidate any outstanding reset tokens once the password changes.
    PasswordResetTokenRepository::new(state.pool())
        .invalidate_for_user(user_id)
        .await
        .map_err(map_identity)?;

    Ok(())
}

/// Always returns success regardless of whether the email exists, to avoid
/// account-enumeration via timing/response differences. When the email maps to
/// an existing credential user we create + email a reset token; otherwise we no-op.
pub(crate) async fn request_password_reset(state: &AppState, email: &str) {
    let Ok(normalized) = validate_email(email) else {
        return;
    };

    let user_repo = UserRepository::new(state.pool());
    let user = match user_repo.fetch_user_by_email(&normalized).await {
        Ok(Some(u)) => u,
        Ok(None) => return,
        Err(e) => {
            tracing::error!(?e, "password reset: user lookup failed");
            return;
        }
    };

    let pw_repo = UserPasswordRepository::new(state.pool());
    if pw_repo.fetch_by_user(user.id).await.ok().flatten().is_none() {
        // OAuth-only user — they should reset via their provider, not us.
        return;
    }

    let (raw_token, token_hash) = generate_reset_token();
    let expires_at = Utc::now() + Duration::hours(RESET_TOKEN_TTL_HOURS);

    let reset_repo = PasswordResetTokenRepository::new(state.pool());
    if let Err(e) = reset_repo.create(user.id, &token_hash, expires_at).await {
        tracing::error!(?e, "password reset: token create failed");
        return;
    }

    let reset_url = format!(
        "{}/auth/password-reset?token={}",
        state.server_public_base_url, raw_token
    );
    state.mailer.send_password_reset(&user.email, &reset_url).await;
}

pub(crate) async fn complete_password_reset(
    state: &AppState,
    payload: &PasswordResetCompleteRequest,
) -> Result<(), CredentialAuthError> {
    validate_password(&payload.new_password)?;

    let token_hash = hex::encode(Sha256::digest(payload.token.as_bytes()));

    let reset_repo = PasswordResetTokenRepository::new(state.pool());
    let token = reset_repo
        .fetch_active(&token_hash)
        .await
        .map_err(map_identity)?
        .ok_or(CredentialAuthError::InvalidResetToken)?;

    let new_hash = hash_password(&payload.new_password)?;
    let pw_repo = UserPasswordRepository::new(state.pool());
    pw_repo
        .upsert(token.user_id, &new_hash, false)
        .await
        .map_err(map_identity)?;

    reset_repo.mark_used(token.id).await.map_err(map_identity)?;
    reset_repo
        .invalidate_for_user(token.user_id)
        .await
        .map_err(map_identity)?;

    // Revoke all active sessions so a leaked session can't outlast the reset.
    AuthSessionRepository::new(state.pool())
        .revoke_all_user_sessions(token.user_id)
        .await
        .map_err(|e| {
            tracing::error!(?e, "password reset: session revoke failed");
            CredentialAuthError::Internal
        })?;

    Ok(())
}

/// Admin-only: create a user with a generated temporary password and add them
/// directly to the org. Bypasses the OAuth-based invitation flow for users
/// whose email has no configured OAuth provider.
pub(crate) async fn provision_member(
    state: &AppState,
    org_id: Uuid,
    admin_user_id: Uuid,
    admin_username: Option<&str>,
    payload: &ProvisionMemberRequest,
) -> Result<ProvisionMemberResponse, CredentialAuthError> {
    assert_admin(state.pool(), org_id, admin_user_id)
        .await
        .map_err(map_identity)?;

    let email = validate_email(&payload.email)?;

    let org_repo = OrganizationRepository::new(state.pool());
    if org_repo.is_personal(org_id).await.map_err(map_identity)? {
        return Err(CredentialAuthError::PersonalOrgInvite);
    }
    let organization = org_repo
        .fetch_organization(org_id)
        .await
        .map_err(map_identity)?;

    let user_repo = UserRepository::new(state.pool());
    let existing = user_repo
        .fetch_user_by_email(&email)
        .await
        .map_err(map_identity)?;

    let user_id = existing.as_ref().map(|u| u.id).unwrap_or_else(Uuid::new_v4);

    // Resolve display fields: explicit payload > preserved existing > derived from email.
    // We materialise to `String` so the resolved values outlive `existing` borrow.
    let first_name_owned: String = payload
        .first_name
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| existing.as_ref().and_then(|u| u.first_name.clone()))
        .unwrap_or_else(|| derive_first_name_from_email(&email));
    let last_name_owned: Option<String> = payload
        .last_name
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| existing.as_ref().and_then(|u| u.last_name.clone()));
    let username_owned: String = payload
        .username
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| existing.as_ref().and_then(|u| u.username.clone()))
        .unwrap_or_else(|| derive_username_from_email(&email));

    let user = user_repo
        .upsert_user(UpsertUser {
            id: user_id,
            email: &email,
            first_name: Some(first_name_owned.as_str()),
            last_name: last_name_owned.as_deref(),
            username: Some(username_owned.as_str()),
        })
        .await
        .map_err(map_identity)?;

    // Personal org for the new user (mirrors the OAuth login flow).
    org_repo
        .ensure_personal_org_and_admin_membership(user.id, Some(username_owned.as_str()))
        .await
        .map_err(map_identity)?;

    let mut tx = crate::db::begin_tx(state.pool()).await.map_err(|e| {
        tracing::error!(?e, "provision_member: begin tx failed");
        CredentialAuthError::Internal
    })?;

    if is_personal_org(&mut *tx, org_id).await.map_err(map_identity)? {
        let _ = tx.rollback().await;
        return Err(CredentialAuthError::PersonalOrgInvite);
    }

    if is_member(&mut *tx, org_id, user.id)
        .await
        .map_err(map_identity)?
    {
        let _ = tx.rollback().await;
        return Err(CredentialAuthError::AlreadyMember);
    }

    add_member(&mut *tx, org_id, user.id, payload.role)
        .await
        .map_err(|e| {
            tracing::error!(?e, "provision_member: add_member failed");
            CredentialAuthError::Internal
        })?;

    tx.commit().await.map_err(|e| {
        tracing::error!(?e, "provision_member: commit failed");
        CredentialAuthError::Internal
    })?;

    let temporary_password = generate_temporary_password();
    let hash = hash_password(&temporary_password)?;
    UserPasswordRepository::new(state.pool())
        .upsert(user.id, &hash, true)
        .await
        .map_err(map_identity)?;

    let login_url = format!("{}/auth/credential-login", state.server_public_base_url);
    state
        .mailer
        .send_credential_invite(
            &organization.name,
            &user.email,
            &login_url,
            &temporary_password,
            payload.role,
            admin_username,
        )
        .await;

    Ok(ProvisionMemberResponse {
        user_id: user.id,
        email: user.email,
        role: payload.role,
        organization_id: org_id,
        temporary_password: if payload.return_password {
            Some(temporary_password)
        } else {
            None
        },
    })
}

#[allow(dead_code)]
pub(crate) fn role_label(role: MemberRole) -> &'static str {
    match role {
        MemberRole::Admin => "admin",
        MemberRole::Member => "member",
    }
}
