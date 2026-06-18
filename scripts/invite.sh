#!/usr/bin/env bash
#
# Create a team-org invitation and print the ready-to-share accept link.
#
# Usage:
#   ./scripts/invite.sh <email> [member|admin]
#
# Config is read from ../.env (run it from the docker/ dir's scripts/, on the
# server that has the .env). Overridable via env:
#   INVITE_BASE         (default: https://$PUBLIC_DOMAIN)
#   INVITE_ORG_ID       (default: $INGEST_ORG_ID)
#   INVITE_ADMIN_EMAIL  (default: $SELF_HOST_LOCAL_AUTH_EMAIL)
#   INVITE_ADMIN_PASSWORD (default: $SELF_HOST_LOCAL_AUTH_PASSWORD)
#
set -euo pipefail

EMAIL="${1:-}"
ROLE="${2:-member}"
[ -n "$EMAIL" ] || { echo "usage: $0 <email> [member|admin]" >&2; exit 2; }
case "$ROLE" in member|admin) ;; *) echo "role must be 'member' or 'admin'" >&2; exit 2;; esac

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# Read a key from .env WITHOUT sourcing it (Docker .env values may contain spaces).
envf() { [ -f "$ROOT/.env" ] && grep -E "^$1=" "$ROOT/.env" | head -1 | cut -d= -f2- || true; }

DOMAIN="${PUBLIC_DOMAIN:-$(envf PUBLIC_DOMAIN)}"
BASE="${INVITE_BASE:-https://${DOMAIN}}"
ORG_ID="${INVITE_ORG_ID:-$(envf INGEST_ORG_ID)}"
ADMIN_EMAIL="${INVITE_ADMIN_EMAIL:-$(envf SELF_HOST_LOCAL_AUTH_EMAIL)}"
ADMIN_PASS="${INVITE_ADMIN_PASSWORD:-$(envf SELF_HOST_LOCAL_AUTH_PASSWORD)}"

[ -n "$BASE" ]    || { echo "INVITE_BASE / PUBLIC_DOMAIN not set" >&2; exit 1; }
[ -n "$ORG_ID" ]  || { echo "INVITE_ORG_ID / INGEST_ORG_ID not set" >&2; exit 1; }
[ -n "$ADMIN_EMAIL" ] && [ -n "$ADMIN_PASS" ] || { echo "admin creds not set (SELF_HOST_LOCAL_AUTH_*)" >&2; exit 1; }

json_get() { sed -E "s/.*\"$1\":\"([^\"]+)\".*/\1/"; }

# 1) admin login (local auth) -> access token
TOKEN=$(curl -fsS -X POST "$BASE/v1/auth/local/login" \
  -H 'content-type: application/json' \
  -d "{\"email\":\"$ADMIN_EMAIL\",\"password\":\"$ADMIN_PASS\"}" | json_get access_token)
[ -n "$TOKEN" ] || { echo "admin login failed" >&2; exit 1; }

# 2) create invitation (API expects role in UPPERCASE: MEMBER | ADMIN)
ROLE_API=$(printf '%s' "$ROLE" | tr '[:lower:]' '[:upper:]')
RESP=$(curl -fsS -X POST "$BASE/v1/organizations/$ORG_ID/invitations" \
  -H "authorization: Bearer $TOKEN" -H 'content-type: application/json' \
  -d "{\"email\":\"$EMAIL\",\"role\":\"$ROLE_API\"}") || { echo "invite request failed" >&2; exit 1; }

TOK=$(printf '%s' "$RESP" | json_get token)
[ -n "$TOK" ] && [ "$TOK" != "$RESP" ] || { echo "could not parse token from: $RESP" >&2; exit 1; }

# 3) print the accept link
echo "✅ Invited $EMAIL as $ROLE (link valid 7 days)."
echo
echo "Send them this link — they sign in with Zoho and accept:"
echo "  $BASE/invitations/$TOK/accept"
