-- Per-user email/password credentials (argon2id hashed).
-- Independent of the single-account SELF_HOST_LOCAL_AUTH_* flow; this enables
-- per-user password auth for invitees whose email has no configured OAuth provider.
CREATE TABLE user_passwords (
    user_id     UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    hash        TEXT NOT NULL,
    must_change BOOLEAN NOT NULL DEFAULT TRUE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Time-limited single-use password reset tokens. token_hash is sha256(raw_token);
-- the raw token is only ever returned in the email and never persisted.
CREATE TABLE password_reset_tokens (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash  TEXT NOT NULL UNIQUE,
    expires_at  TIMESTAMPTZ NOT NULL,
    used_at     TIMESTAMPTZ,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX password_reset_tokens_active_idx
    ON password_reset_tokens (token_hash)
    WHERE used_at IS NULL;

CREATE INDEX password_reset_tokens_user_idx
    ON password_reset_tokens (user_id);
