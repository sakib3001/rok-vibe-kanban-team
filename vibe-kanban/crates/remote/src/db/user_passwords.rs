use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use super::identity_errors::IdentityError;

#[derive(Debug, Clone)]
pub struct UserPassword {
    pub user_id: Uuid,
    pub hash: String,
    pub must_change: bool,
    pub updated_at: DateTime<Utc>,
}

pub struct UserPasswordRepository<'a> {
    pool: &'a PgPool,
}

impl<'a> UserPasswordRepository<'a> {
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }

    pub async fn fetch_by_user(
        &self,
        user_id: Uuid,
    ) -> Result<Option<UserPassword>, IdentityError> {
        let row = sqlx::query!(
            r#"
            SELECT
                user_id     AS "user_id!: Uuid",
                hash        AS "hash!",
                must_change AS "must_change!",
                updated_at  AS "updated_at!"
            FROM user_passwords
            WHERE user_id = $1
            "#,
            user_id
        )
        .fetch_optional(self.pool)
        .await?;

        Ok(row.map(|r| UserPassword {
            user_id: r.user_id,
            hash: r.hash,
            must_change: r.must_change,
            updated_at: r.updated_at,
        }))
    }

    pub async fn upsert(
        &self,
        user_id: Uuid,
        hash: &str,
        must_change: bool,
    ) -> Result<(), IdentityError> {
        sqlx::query!(
            r#"
            INSERT INTO user_passwords (user_id, hash, must_change)
            VALUES ($1, $2, $3)
            ON CONFLICT (user_id) DO UPDATE
            SET hash = EXCLUDED.hash,
                must_change = EXCLUDED.must_change,
                updated_at = NOW()
            "#,
            user_id,
            hash,
            must_change
        )
        .execute(self.pool)
        .await?;

        Ok(())
    }

    pub async fn clear_must_change(&self, user_id: Uuid) -> Result<(), IdentityError> {
        sqlx::query!(
            r#"
            UPDATE user_passwords
            SET must_change = FALSE,
                updated_at = NOW()
            WHERE user_id = $1
            "#,
            user_id
        )
        .execute(self.pool)
        .await?;

        Ok(())
    }
}
