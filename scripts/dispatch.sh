#!/usr/bin/env bash
#
# Lead dispatch CLI for assignment operations against the remote API.
#
# Commands:
#   ./scripts/dispatch.sh list-unassigned <project_id> [--limit N]
#   ./scripts/dispatch.sh list-by-dev <project_id> <email|user_id> [--limit N]
#   ./scripts/dispatch.sh assign <issue_id> <email|user_id>
#   ./scripts/dispatch.sh bulk-assign <project_id> <email|user_id> [--limit N]
#
# Env defaults (read from .env when omitted):
#   DISPATCH_BASE            -> https://$PUBLIC_DOMAIN
#   DISPATCH_ORG_ID          -> INGEST_ORG_ID
#   DISPATCH_ADMIN_EMAIL     -> SELF_HOST_LOCAL_AUTH_EMAIL
#   DISPATCH_ADMIN_PASSWORD  -> SELF_HOST_LOCAL_AUTH_PASSWORD
#
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ENV_FILE="${ROOT}/.env"

usage() {
  cat <<'USAGE'
Usage:
  scripts/dispatch.sh list-unassigned <project_id> [--limit N]
  scripts/dispatch.sh list-by-dev <project_id> <email|user_id> [--limit N]
  scripts/dispatch.sh assign <issue_id> <email|user_id>
  scripts/dispatch.sh bulk-assign <project_id> <email|user_id> [--limit N]

Examples:
  ./scripts/dispatch.sh list-unassigned 63051e48-a41b-4242-8c67-138b24e7114a
  ./scripts/dispatch.sh list-by-dev 63051e48-a41b-4242-8c67-138b24e7114a sakib@rokomari.com
  ./scripts/dispatch.sh assign 23c5f9b4-2465-4567-b8ea-ba9af985fe61 sakib@rokomari.com
  ./scripts/dispatch.sh bulk-assign 63051e48-a41b-4242-8c67-138b24e7114a joy@rokomari.com --limit 10
USAGE
}

envf() {
  local key="$1"
  [ -f "$ENV_FILE" ] && grep -E "^${key}=" "$ENV_FILE" | head -1 | cut -d= -f2- || true
}

BASE="${DISPATCH_BASE:-}"
if [ -z "$BASE" ]; then
  DOMAIN="${PUBLIC_DOMAIN:-$(envf PUBLIC_DOMAIN)}"
  BASE="https://${DOMAIN}"
fi

ORG_ID="${DISPATCH_ORG_ID:-$(envf INGEST_ORG_ID)}"
ADMIN_EMAIL="${DISPATCH_ADMIN_EMAIL:-$(envf SELF_HOST_LOCAL_AUTH_EMAIL)}"
ADMIN_PASSWORD="${DISPATCH_ADMIN_PASSWORD:-$(envf SELF_HOST_LOCAL_AUTH_PASSWORD)}"

if [ -z "$BASE" ] || [ -z "$ORG_ID" ] || [ -z "$ADMIN_EMAIL" ] || [ -z "$ADMIN_PASSWORD" ]; then
  echo "Missing dispatch config. Need BASE, ORG_ID, admin email/password (from env or .env)." >&2
  exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
  echo "python3 is required for scripts/dispatch.sh" >&2
  exit 1
fi

python3 - "$BASE" "$ORG_ID" "$ADMIN_EMAIL" "$ADMIN_PASSWORD" "$@" <<'PY'
import json
import re
import sys
import uuid
import urllib.error
import urllib.request

BASE, ORG_ID, ADMIN_EMAIL, ADMIN_PASSWORD, *argv = sys.argv[1:]

def fail(msg: str, code: int = 1):
    print(msg, file=sys.stderr)
    raise SystemExit(code)

def request(path: str, method: str = "GET", token: str | None = None, payload=None):
    url = f"{BASE.rstrip('/')}{path}"
    data = None
    headers = {"accept": "application/json"}
    if payload is not None:
        data = json.dumps(payload).encode("utf-8")
        headers["content-type"] = "application/json"
    if token:
        headers["authorization"] = f"Bearer {token}"
    req = urllib.request.Request(url, data=data, method=method, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=20) as res:
            body = res.read().decode("utf-8")
            return res.status, body
    except urllib.error.HTTPError as e:
        body = e.read().decode("utf-8", errors="ignore")
        return e.code, body

def parse_json(body: str):
    try:
        return json.loads(body)
    except Exception:
        return {}

def login():
    status, body = request(
        "/v1/auth/local/login",
        method="POST",
        payload={"email": ADMIN_EMAIL, "password": ADMIN_PASSWORD},
    )
    if status != 200:
        fail(f"Login failed: HTTP {status} {body}")
    j = parse_json(body)
    tok = j.get("access_token")
    if not tok:
        fail(f"Login response missing access_token: {body}")
    return tok

def get_members(token: str):
    status, body = request(f"/v1/organizations/{ORG_ID}/members", token=token)
    if status != 200:
        fail(f"Failed to list org members: HTTP {status} {body}")
    j = parse_json(body)
    return j.get("members") or j.get("data") or []

UUID_RE = re.compile(
    r"^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$", re.I
)

def resolve_user_id(token: str, ident: str) -> str:
    if UUID_RE.match(ident):
        return ident
    target = ident.strip().lower()
    members = get_members(token)
    for m in members:
        if str(m.get("email", "")).strip().lower() == target:
            return m["user_id"]
    fail(f"No org member found for email: {ident}")

def search_issues(token: str, project_id: str, assignee_user_id: str | None, limit: int):
    payload = {
        "project_id": project_id,
        "status_id": None,
        "status_ids": None,
        "priority": None,
        "parent_issue_id": None,
        "search": None,
        "simple_id": None,
        "assignee_user_id": assignee_user_id,
        "tag_id": None,
        "tag_ids": None,
        "sort_field": "updated_at",
        "sort_direction": "desc",
        "limit": limit,
        "offset": 0,
    }
    status, body = request("/v1/issues/search", method="POST", token=token, payload=payload)
    if status != 200:
        fail(f"Failed to search issues: HTTP {status} {body}")
    j = parse_json(body)
    return j.get("issues") or j.get("data") or []

def list_issue_assignees(token: str, issue_id: str):
    status, body = request(f"/v1/issue_assignees?issue_id={issue_id}", token=token)
    if status != 200:
        return []
    j = parse_json(body)
    return j.get("issue_assignees") or j.get("data") or []

def assign_issue(token: str, issue_id: str, user_id: str):
    payload = {"id": str(uuid.uuid4()), "issue_id": issue_id, "user_id": user_id}
    status, body = request("/v1/issue_assignees", method="POST", token=token, payload=payload)
    if status not in (200, 201):
        fail(f"Assign failed for issue {issue_id}: HTTP {status} {body}")

def print_issues(issues):
    if not issues:
        print("No issues found.")
        return
    for issue in issues:
        simple = issue.get("simple_id") or issue.get("issue_number") or "?"
        title = issue.get("title", "").strip()
        status_id = issue.get("status_id", "")
        priority = issue.get("priority") or "-"
        print(f"- {simple} | {priority:<6} | status={status_id} | {title} | id={issue.get('id')}")

def parse_limit(args):
    limit = 200
    if "--limit" in args:
        idx = args.index("--limit")
        try:
            limit = int(args[idx + 1])
        except Exception:
            fail("--limit requires an integer value")
    return limit

if not argv:
    usage = "No command provided. Use: list-unassigned | list-by-dev | assign | bulk-assign"
    fail(usage, 2)

cmd = argv[0]
token = login()

if cmd == "list-unassigned":
    if len(argv) < 2:
        fail("Usage: list-unassigned <project_id> [--limit N]", 2)
    project_id = argv[1]
    limit = parse_limit(argv[2:])
    issues = search_issues(token, project_id, assignee_user_id=None, limit=limit)
    unassigned = []
    for issue in issues:
        assignees = list_issue_assignees(token, issue["id"])
        if len(assignees) == 0:
            unassigned.append(issue)
    print_issues(unassigned)
elif cmd == "list-by-dev":
    if len(argv) < 3:
        fail("Usage: list-by-dev <project_id> <email|user_id> [--limit N]", 2)
    project_id = argv[1]
    ident = argv[2]
    limit = parse_limit(argv[3:])
    user_id = resolve_user_id(token, ident)
    issues = search_issues(token, project_id, assignee_user_id=user_id, limit=limit)
    print_issues(issues)
elif cmd == "assign":
    if len(argv) != 3:
        fail("Usage: assign <issue_id> <email|user_id>", 2)
    issue_id, ident = argv[1], argv[2]
    user_id = resolve_user_id(token, ident)
    assign_issue(token, issue_id, user_id)
    print(f"Assigned issue {issue_id} -> {user_id}")
elif cmd == "bulk-assign":
    if len(argv) < 3:
        fail("Usage: bulk-assign <project_id> <email|user_id> [--limit N]", 2)
    project_id, ident = argv[1], argv[2]
    limit = parse_limit(argv[3:])
    user_id = resolve_user_id(token, ident)
    issues = search_issues(token, project_id, assignee_user_id=None, limit=limit)
    assigned = 0
    for issue in issues:
        if len(list_issue_assignees(token, issue["id"])) > 0:
            continue
        assign_issue(token, issue["id"], user_id)
        assigned += 1
        simple = issue.get("simple_id") or issue.get("issue_number") or "?"
        print(f"assigned {simple} ({issue['id']}) -> {user_id}")
    print(f"Done. Assigned {assigned} issue(s).")
else:
    fail(f"Unknown command: {cmd}", 2)
PY
