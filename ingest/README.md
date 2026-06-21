# Issue-ingestion sidecar

`POST /ingest/issues` → auto-creates an issue on the central server.

`POST /ingest/requirements/drafts` → creates an approval-gated requirement draft (agent-centric flow) that a human must approve before publishing epic + child tasks.

A small Node service (no external deps) that authenticates as a **service account** and calls
the existing `/v1/issues` API. Enabled via the `ingest` compose profile; routed by Caddy at
`https://vk.rokomari.io/ingest/*`.

```
caller → Caddy (/ingest/*) → ingest:8090 → remote:8081 (/v1/issues, as service account)
```

> **Caller-facing reference (curl examples + schema): [API.md](./API.md)** — hand this to
> whoever builds the calling integration. This README is operator/setup focused.

## API

`POST /ingest/issues`

| Header | Value |
|--------|-------|
| `X-API-Key` | your `INGEST_API_KEY` (or `Authorization: Bearer <key>`) |
| `Content-Type` | `application/json` |

Body (we define this schema):

```json
{
  "title": "Disk usage high on db-1",      // required
  "description": "node exporter > 90%",     // optional
  "priority": "High",                        // optional: Urgent | High | Medium | Low
  "dedup_key": "alert-12345",                // optional: repeat posts return the same issue
  "assignee": "dev@rokomari.com"             // optional: email of a team-org member
}
```

Responses: `201 {created:true, id, url}` · `200 {deduped:true, id}` (dedup_key seen before) ·
`400` (missing title / bad priority / bad JSON) · `401` (bad key) · `502` (upstream create failed).

`GET /health` → `200 {status:"ok"}`.

### Agent-centric requirement workflow (new)

0. (Optional) Upload source file through ingest:
   - `PUT /ingest/requirements/sources/{object_key}` (raw bytes), or
   - `POST /ingest/requirements/sources/upload` (multipart form).
1. Agent submits requirement draft to `POST /ingest/requirements/drafts`.
2. Human reviews via `GET /ingest/requirements/drafts` or `GET /ingest/requirements/drafts/{id}`.
3. Human explicitly approves: `POST /ingest/requirements/drafts/{id}/approve`.
4. Ingest publishes epic + child tasks.
5. Reject path: `POST /ingest/requirements/drafts/{id}/reject`.
6. For private R2 source files, fetch fresh access links via `GET /ingest/requirements/drafts/{id}/signed-source-links`.

For payloads and examples, see [`API.md`](./API.md).

### Example

```bash
curl -X POST https://vk.rokomari.io/ingest/issues \
  -H "X-API-Key: $INGEST_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"title":"Build broke on main","description":"step X failed","priority":"High","dedup_key":"ci-build-9981"}'
```

## One-time setup

### 1. Service-account credentials (self-host local auth)
The sidecar logs in via the remote's **local auth**, so the **remote** must be configured
with the bot credentials. In `docker/.env`:

```bash
# remote service reads these (single local-auth slot — used by the bot)
SELF_HOST_LOCAL_AUTH_EMAIL=issue-bot@rokomari.com
SELF_HOST_LOCAL_AUTH_PASSWORD=<strong-password>

# ingest service uses the SAME creds + an API key callers must send
INGEST_SVC_EMAIL=issue-bot@rokomari.com
INGEST_SVC_PASSWORD=<strong-password>
INGEST_API_KEY=$(openssl rand -hex 32)
```

Restart remote so it picks up local-auth: `docker compose up -d remote`.

### 2. Create the bot user + add it to the team org
The bot must be a **member of the org that owns the target project** (local-auth users only
get a personal org by default). Create the user by logging in once, then grant membership:

```bash
# (a) trigger one login so the user row is created
docker compose --profile ingest up -d ingest
curl -s -XPOST https://vk.rokomari.io/ingest/issues -H "X-API-Key: $INGEST_API_KEY" \
  -H 'content-type: application/json' -d '{"title":"__bootstrap__"}'   # will 502 until step (b)

# (b) find ids and grant membership (self-hosted: direct SQL is the reliable path)
docker compose exec postgres psql -U remote -d remote -c \
 "SELECT id,email FROM users WHERE email='issue-bot@rokomari.com';"
docker compose exec postgres psql -U remote -d remote -c \
 "SELECT o.id AS org_id,o.name,p.id AS project_id,p.name FROM organizations o JOIN projects p ON p.organization_id=o.id WHERE o.name='Rokomari SE Team';"

docker compose exec postgres psql -U remote -d remote -c \
 "INSERT INTO organization_member_metadata (organization_id,user_id,role) \
  VALUES ('<org_id>','<bot_user_id>','member') ON CONFLICT DO NOTHING;"
```

### 3. Point ingest at the project
Set `INGEST_PROJECT_ID=<project_id>` in `.env`. The default status is picked by name
(`INGEST_STATUS_NAME=todo`) or pin an exact `INGEST_STATUS_ID`. Then:

```bash
docker compose --profile ingest up -d ingest
docker compose logs -f ingest        # expect "service account logged in" + "default status -> ..."
```

## Configuration (env)

| Var | Required | Default | Notes |
|-----|----------|---------|-------|
| `INGEST_API_KEY` | ✅ | — | secret callers must send |
| `INGEST_SVC_EMAIL` / `INGEST_SVC_PASSWORD` | ✅ | — | must match the remote's local-auth creds |
| `INGEST_PROJECT_ID` | ✅ | — | target project UUID (bot must have access) |
| `INGEST_ORG_ID` | for `assignee` | — | team org UUID; needed to resolve `assignee` emails → user IDs |
| `INGEST_STATUS_NAME` | | `todo` | status name substring to match |
| `INGEST_STATUS_ID` | | — | pin exact status UUID (overrides name match) |
| `INGEST_PUBLIC_URL` | | `https://<PUBLIC_DOMAIN>` | for building issue links in responses |
| `REMOTE_URL` | | `http://remote:8081` | internal address of the remote API |
| `INGEST_DEDUP_FILE` | | `/data/dedup.json` | persisted on the `ingest_data` volume |
| `INGEST_REQUIREMENTS_FILE` | | `/data/requirements-drafts.json` | persisted draft + revision state |
| `INGEST_MAX_BODY_KB` | | `2048` | max request size for ingest endpoints |
| `INGEST_MAX_CHILD_TASKS` | | `12` | max auto-published child tasks per epic draft |
| `R2_ACCESS_KEY_ID` / `R2_SECRET_ACCESS_KEY` | for private sources | — | needed to generate signed URLs for `source.object_keys` |
| `R2_REVIEW_ENDPOINT` / `R2_REVIEW_BUCKET` | for private sources | — | endpoint + bucket used for source-link signing |
| `R2_REGION` | | `auto` | AWS signing region for R2 |
| `R2_PRESIGN_EXPIRY_SECS` | | `300` | signed link expiry (seconds) |
| `INGEST_UPLOAD_MAX_MB` | | `20` | max upload body size for source uploads |

## Notes
- **Dedup** is by `dedup_key`, persisted to a volume; omit the key to always create a new issue.
- The single local-auth slot is consumed by the bot — humans still sign in via OAuth.
- Tokens are obtained via local login and auto-refreshed; on 401 the sidecar refreshes/re-logs in.
- Requirement drafts are persisted and revisioned by `source.fingerprint`.
- Private source docs should be provided as `source.object_keys` and accessed using short-lived signed URLs.
