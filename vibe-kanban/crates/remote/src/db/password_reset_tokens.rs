use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use super::identity_errors::IdentityError;

#[derive(Debug, Clone)]
pub struct PasswordResetToken {
    pub id: Uuid,
    pub user_id: Uuid,
    pub token_hash: String,
    pub expires_at: DateTime<Utc>,
    pub used_at: Option<DateTime<Utc>>,
}

pub struct PasswordResetTokenRepository<'a> {
    pool: &'a PgPool,
}

impl<'a> PasswordResetTokenRepository<'a> {
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }

    pub async fn create(
        &self,
        user_id: Uuid,
        token_hash: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<Uuid, IdentityError> {
        let row = sqlx::query!(
            r#"
            INSERT INTO password_reset_tokens (user_id, token_hash, expires_at)
            VALUES ($1, $2, $3)
            RETURNING id AS "id!: Uuid"
            "#,
            user_id,
            token_hash,
            expires_at
        )
        .fetch_one(self.pool)
        .await?;

        Ok(row.id)
    }

    /// Look up an unused, unexpired reset token by its hashed form.
    pub async fn fetch_active(
        &self,
        token_hash: &str,
    ) -> Result<Option<PasswordResetToken>, IdentityError> {
        let row = sqlx::query!(
            r#"
            SELECT
                id          AS "id!: Uuid",
                user_id     AS "user_id!: Uuid",
                token_hash  AS "token_hash!",
                expires_at  AS "expires_at!",
                used_at     AS "used_at?"
            FROM password_reset_tokens
            WHERE token_hash = $1
              AND used_at IS NULL
              AND expires_at > NOW()
            "#,
            token_hash
        )
        .fetch_optional(self.pool)
        .await?;

        Ok(row.map(|r| PasswordResetToken {
            id: r.id,
            user_id: r.user_id,
            token_hash: r.token_hash,
            expires_at: r.expires_at,
            used_at: r.used_at,
        }))
    }

    pub async fn mark_used(&self, id: Uuid) -> Result<(), IdentityError> {
        sqlx::query!(
            r#"
            UPDATE password_reset_tokens
            SET used_at = NOW()
            WHERE id = $1
            "#,
            id
        )
        .execute(self.pool)
        .await?;

        Ok(())
    }

    /// Invalidate any outstanding reset tokens for a user. Called after a
    /// successful password change/reset so a leaked token cannot be reused.
    pub async fn invalidate_for_user(&self, user_id: Uuid) -> Result<(), IdentityError> {
        sqlx::query!(
            r#"
            UPDATE password_reset_tokens
            SET used_at = NOW()
            WHERE user_id = $1
              AND used_at IS NULL
            "#,
            user_id
        )
        .execute(self.pool)
        .await?;

        Ok(())
    }
}
