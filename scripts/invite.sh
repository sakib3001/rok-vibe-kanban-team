#!/usr/bin/env bash
#
# Team-org user management CLI (run manually on the server that has .env).
#
# Subcommands:
#   create <email> [member|admin] [--name "First Last"] [--username handle]
#       Provision a user directly: creates the account, adds them to the org,
#       generates a temporary password, and PRINTS it for you to hand over.
#       The user logs in with email + password and must change it on first login.
#       (No email delivery is relied upon — creds are returned in-band.)
#
#   passwd <email|user_id> [member|admin]
#       Reset an existing member's password. There is no admin "set password"
#       API, so this removes the membership and re-provisions, yielding a NEW
#       temporary password (role preserved unless overridden). Prints the creds.
#
#   delete <email> [--yes]
#       HARD-DELETE the user account from the database (irreversible). Cascades
#       to memberships, assignees, comments-authorship (nulled), sessions, etc.
#       Requires confirmation unless --yes is given. Runs psql via docker compose.
#
#   link <email> [member|admin]
#       Classic OAuth invitation: prints a 7-day accept link. Use for users who
#       sign in via Zoho/GitHub/Google.
#
#   list
#       List current org members (email | role | user_id).
#
# Config is read from ../.env (run from the docker/ dir's scripts/). Overridable:
#   INVITE_BASE (https://$PUBLIC_DOMAIN), INVITE_ORG_ID ($INGEST_ORG_ID),
#   INVITE_ADMIN_EMAIL ($SELF_HOST_LOCAL_AUTH_EMAIL),
#   INVITE_ADMIN_PASSWORD ($SELF_HOST_LOCAL_AUTH_PASSWORD),
#   PGUSER (remote), PGDB (remote).
#
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# Read a key from .env WITHOUT sourcing it (Docker .env values may contain spaces).
envf() { [ -f "$ROOT/.env" ] && grep -E "^$1=" "$ROOT/.env" | head -1 | cut -d= -f2- || true; }

DOMAIN="${PUBLIC_DOMAIN:-$(envf PUBLIC_DOMAIN)}"
BASE="${INVITE_BASE:-https://${DOMAIN}}"
ORG_ID="${INVITE_ORG_ID:-$(envf INGEST_ORG_ID)}"
ADMIN_EMAIL="${INVITE_ADMIN_EMAIL:-$(envf SELF_HOST_LOCAL_AUTH_EMAIL)}"
ADMIN_PASS="${INVITE_ADMIN_PASSWORD:-$(envf SELF_HOST_LOCAL_AUTH_PASSWORD)}"
PGUSER="${PGUSER:-remote}"
PGDB="${PGDB:-remote}"

usage() {
  sed -n '3,40p' "$0" | sed 's/^# \{0,1\}//'
  exit "${1:-2}"
}

# Extract a JSON string field, returning EMPTY if absent (so failures are detectable).
# Uses jq when available, else a field-scoped grep (not a whole-line passthrough).
json_get() {
  if command -v jq >/dev/null 2>&1; then
    jq -r --arg k "$1" '.. | objects | select(has($k)) | .[$k] | strings' 2>/dev/null | head -1
  else
    grep -oE "\"$1\"[[:space:]]*:[[:space:]]*\"[^\"]*\"" | head -1 | sed -E "s/.*:[[:space:]]*\"([^\"]*)\"/\1/"
  fi
}

need_cfg() {
  [ -n "$BASE" ]        || { echo "INVITE_BASE / PUBLIC_DOMAIN not set" >&2; exit 1; }
  [ -n "$ORG_ID" ]      || { echo "INVITE_ORG_ID / INGEST_ORG_ID not set" >&2; exit 1; }
  [ -n "$ADMIN_EMAIL" ] && [ -n "$ADMIN_PASS" ] || { echo "admin creds not set (SELF_HOST_LOCAL_AUTH_*)" >&2; exit 1; }
}

TOKEN=""
login() {
  TOKEN=$(curl -fsS -X POST "$BASE/v1/auth/local/login" \
    -H 'content-type: application/json' \
    -d "$(printf '{"email":"%s","password":"%s"}' "$ADMIN_EMAIL" "$ADMIN_PASS")" | json_get access_token)
  [ -n "$TOKEN" ] || { echo "admin login failed" >&2; exit 1; }
}

# jq-built JSON body so values are escaped safely.
provision_body() { # email role first last username
  jq -nc \
    --arg email "$1" --arg role "$2" --arg first "$3" --arg last "$4" --arg user "$5" \
    '{email:$email, role:$role, return_password:true}
     + (if $first|length>0 then {first_name:$first} else {} end)
     + (if $last |length>0 then {last_name:$last}  else {} end)
     + (if $user |length>0 then {username:$user}   else {} end)'
}

# email|user_id -> "user_id<TAB>role" from the org member list; empty if not a member.
resolve_member() {
  local ident="$1"
  curl -fsS "$BASE/v1/organizations/$ORG_ID/members" -H "authorization: Bearer $TOKEN" \
    | jq -r --arg q "$ident" '
        (.members // .data // [])[]
        | select((.email|ascii_downcase == ($q|ascii_downcase)) or (.user_id == $q))
        | "\(.user_id)\t\(.role)"' | head -1
}

upper() { printf '%s' "$1" | tr '[:lower:]' '[:upper:]'; }
valid_email() { printf '%s' "$1" | grep -qE '^[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}$'; }

print_creds() { # email password
  echo "✅ Done."
  echo
  echo "Hand these to the user (they must change the password on first login):"
  echo "  login:    $BASE"
  echo "  email:    $1"
  echo "  password: $2"
}

cmd_create() {
  local email="" role="member" name="" username=""
  while [ $# -gt 0 ]; do
    case "$1" in
      --name) name="${2:-}"; shift ;;        --name=*) name="${1#--name=}" ;;
      --username) username="${2:-}"; shift ;; --username=*) username="${1#--username=}" ;;
      member|admin) role="$1" ;;
      -*) echo "unknown flag: $1" >&2; exit 2 ;;
      *) [ -z "$email" ] && email="$1" || { echo "unexpected arg: $1" >&2; exit 2; } ;;
    esac
    shift
  done
  [ -n "$email" ] || usage
  valid_email "$email" || { echo "invalid email: $email" >&2; exit 2; }
  local first="" last=""
  if [ -n "$name" ]; then first="${name%% *}"; [ "$name" != "$first" ] && last="${name#* }"; fi
  login
  local resp; resp=$(curl -sS -X POST "$BASE/v1/organizations/$ORG_ID/members/provision" \
    -H "authorization: Bearer $TOKEN" -H 'content-type: application/json' \
    -d "$(provision_body "$email" "$(upper "$role")" "$first" "$last" "$username")")
  local pw; pw=$(printf '%s' "$resp" | json_get temporary_password)
  if [ -z "$pw" ]; then
    local err; err=$(printf '%s' "$resp" | json_get error)
    if [ "$err" = "already_member" ]; then
      echo "⚠️  $email is already a member. To reset their password run: $0 passwd $email" >&2
    else
      echo "provision failed: $resp" >&2
    fi
    exit 1
  fi
  echo "✅ Created $email as $role."; echo
  print_creds "$email" "$pw"
}

cmd_passwd() {
  local ident="${1:-}"; shift || true
  local role_override=""
  [ "${1:-}" = "member" ] || [ "${1:-}" = "admin" ] && role_override="${1:-}"
  [ -n "$ident" ] || usage
  login
  local row uid role; row=$(resolve_member "$ident")
  [ -n "$row" ] || { echo "no org member matches: $ident (use '$0 create' for new users)" >&2; exit 1; }
  uid="${row%%$'\t'*}"; role="${row#*$'\t'}"
  [ -n "$role_override" ] && role="$role_override"
  # Need the email for the re-provision call; resolve_member matched it, but the
  # caller may have passed a user_id — fetch the email from the member list.
  local email; email=$(curl -fsS "$BASE/v1/organizations/$ORG_ID/members" -H "authorization: Bearer $TOKEN" \
    | jq -r --arg u "$uid" '(.members // .data // [])[] | select(.user_id==$u) | .email' | head -1)
  [ -n "$email" ] || { echo "could not resolve email for user $uid" >&2; exit 1; }
  echo "Resetting password for $email (role $role) — removing + re-provisioning..."
  local code; code=$(curl -sS -o /dev/null -w '%{http_code}' -X DELETE \
    "$BASE/v1/organizations/$ORG_ID/members/$uid" -H "authorization: Bearer $TOKEN")
  [ "$code" = "204" ] || { echo "could not remove membership (HTTP $code) — sole admin or self? aborting; password unchanged." >&2; exit 1; }
  local resp; resp=$(curl -sS -X POST "$BASE/v1/organizations/$ORG_ID/members/provision" \
    -H "authorization: Bearer $TOKEN" -H 'content-type: application/json' \
    -d "$(provision_body "$email" "$(upper "$role")" "" "" "")")
  local pw; pw=$(printf '%s' "$resp" | json_get temporary_password)
  [ -n "$pw" ] || { echo "RE-PROVISION FAILED after removal: $resp" >&2; echo "User $email is now removed — re-run: $0 create $email $role" >&2; exit 1; }
  echo "✅ Password reset for $email."; echo
  print_creds "$email" "$pw"
}

# `-T` reads stdin; redirect from /dev/null so it never swallows the terminal
# (interactive confirmation prompt) or a calling heredoc.
psql_exec() { ( cd "$ROOT" && docker compose exec -T postgres psql -U "$PGUSER" -d "$PGDB" "$@" </dev/null ); }

cmd_delete() {
  local email="" yes=0
  while [ $# -gt 0 ]; do
    case "$1" in
      --yes|-y) yes=1 ;;
      -*) echo "unknown flag: $1" >&2; exit 2 ;;
      *) [ -z "$email" ] && email="$1" || { echo "unexpected arg: $1" >&2; exit 2; } ;;
    esac
    shift
  done
  [ -n "$email" ] || usage
  valid_email "$email" || { echo "invalid email: $email" >&2; exit 2; }
  if [ "$(printf '%s' "$email" | tr '[:upper:]' '[:lower:]')" = "$(printf '%s' "$ADMIN_EMAIL" | tr '[:upper:]' '[:lower:]')" ]; then
    echo "refusing to delete the local admin account ($ADMIN_EMAIL)" >&2; exit 1
  fi
  local uid; uid=$(psql_exec -At -c "select id from users where lower(email)=lower('$email');")
  [ -n "$uid" ] || { echo "no user with email: $email" >&2; exit 1; }
  echo "⚠️  HARD DELETE — irreversibly removes user $email ($uid) and all owned data."
  if [ "$yes" -ne 1 ]; then
    printf 'Type the email again to confirm: '
    read -r confirm
    [ "$confirm" = "$email" ] || { echo "confirmation mismatch — aborted." >&2; exit 1; }
  fi
  # oauth_handoffs FK has no ON DELETE rule; clear it first, then the cascade does the rest.
  psql_exec -q -c "delete from oauth_handoffs where user_id='$uid';" \
                 -c "delete from users where id='$uid';"
  echo "✅ Deleted $email ($uid)."
}

cmd_link() {
  local email="${1:-}" role="${2:-member}"
  [ -n "$email" ] || usage
  case "$role" in member|admin) ;; *) echo "role must be member|admin" >&2; exit 2 ;; esac
  login
  local resp; resp=$(curl -fsS -X POST "$BASE/v1/organizations/$ORG_ID/invitations" \
    -H "authorization: Bearer $TOKEN" -H 'content-type: application/json' \
    -d "$(printf '{"email":"%s","role":"%s"}' "$email" "$(upper "$role")")") \
    || { echo "invite request failed" >&2; exit 1; }
  local tok; tok=$(printf '%s' "$resp" | json_get token)
  [ -n "$tok" ] || { echo "could not parse token from: $resp" >&2; exit 1; }
  echo "✅ Invited $email as $role (link valid 7 days)."; echo
  echo "Send them this link — they sign in via OAuth and accept:"
  echo "  $BASE/invitations/$tok/accept"
}

cmd_list() {
  login
  echo "EMAIL                                    ROLE     USER_ID"
  curl -fsS "$BASE/v1/organizations/$ORG_ID/members" -H "authorization: Bearer $TOKEN" \
    | jq -r '(.members // .data // [])[] | "\(.email)\t\(.role)\t\(.user_id)"' \
    | awk -F'\t' '{printf "%-40s %-8s %s\n", $1, $2, $3}'
}

CMD="${1:-}"; shift || true
case "$CMD" in
  create) need_cfg; cmd_create "$@" ;;
  passwd) need_cfg; cmd_passwd "$@" ;;
  delete) need_cfg; cmd_delete "$@" ;;
  link)   need_cfg; cmd_link "$@" ;;
  list)   need_cfg; cmd_list "$@" ;;
  -h|--help|help|"") usage 0 ;;
  *) echo "unknown command: $CMD" >&2; usage ;;
esac
