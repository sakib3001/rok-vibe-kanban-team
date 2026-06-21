# Vibe Kanban — Issue Ingestion API

Create issues on the central board (`https://vk.rokomari.io`) by POSTing JSON.
Hand this file to whoever builds the calling integration.

There are now two ingestion modes:

1. **Direct issue mode** (existing): `POST /ingest/issues` creates an issue immediately.
2. **Agent-centric requirement mode** (new): create a draft, require human approval, then publish epic + child tasks.

For private source files, ingest also supports direct upload to R2 (PUT or multipart).

## Endpoint

```
POST https://vk.rokomari.io/ingest/issues
```

## Auth

Send the shared secret as a header (either form works):

```
X-API-Key: <INGEST_API_KEY>
# or
Authorization: Bearer <INGEST_API_KEY>
```

The key is held by the platform team (stored server-side in `docker/.env`).

## Request body

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `title` | string | ✅ | issue title |
| `description` | string | | issue body |
| `priority` | string | | `Urgent` \| `High` \| `Medium` \| `Low` (case-insensitive) |
| `dedup_key` | string | | repeat posts with the same key return the existing issue (idempotent) |
| `assignee` | string | | **email** of a team-org member to assign the issue to |

Issues are created in the **Amaly** project (Rokomari SE Team), status **To do**.
Unassigned unless `assignee` is given.

**Assignee handling:** `assignee` must be the email of a member of the team org. If it
doesn't resolve (typo / not a member), the issue is **still created unassigned** and the
response reports `assignee.resolved:false` with a reason — assignment never blocks issue
creation. On success the response includes `assignee.resolved:true` and the `user_id`.

## Responses

| Status | Body | Meaning |
|--------|------|---------|
| `201` | `{"created":true,"id":"…","url":"…"}` (plus `"assignee":{"resolved":true/false,…}` when `assignee` was sent) | issue created |
| `200` | `{"deduped":true,"id":"…"}` | `dedup_key` already used → existing issue returned |
| `400` | `{"error":"…"}` | missing `title`, bad `priority`, or invalid JSON |
| `401` | `{"error":"unauthorized"}` | missing/wrong API key |
| `502` | `{"error":"…"}` | upstream issue creation failed |

## curl examples

```bash
KEY=<INGEST_API_KEY>
EP=https://vk.rokomari.io/ingest/issues
```

```bash
# all fields together -> 201
curl -i -X POST $EP -H "X-API-Key: $KEY" -H 'content-type: application/json' \
  -d '{"title":"DB latency spike","description":"p99 > 2s for 5m","priority":"High","assignee":"dev@rokomari.com","dedup_key":"alert-db-latency"}'

# minimal (title only) -> 201
curl -i -X POST $EP -H "X-API-Key: $KEY" -H 'content-type: application/json' \
  -d '{"title":"Issue from API"}'

# full: description + priority + dedup_key -> 201
curl -i -X POST $EP -H "X-API-Key: $KEY" -H 'content-type: application/json' \
  -d '{"title":"Disk high on db-1","description":"node_exporter > 90%","priority":"High","dedup_key":"alert-12345"}'

# dedup: same dedup_key again -> 200 {"deduped":true,...} (same id)
curl -i -X POST $EP -H "X-API-Key: $KEY" -H 'content-type: application/json' \
  -d '{"title":"ignored","dedup_key":"alert-12345"}'

# priority values: Urgent | High | Medium | Low (case-insensitive)
curl -i -X POST $EP -H "X-API-Key: $KEY" -H 'content-type: application/json' \
  -d '{"title":"urgent thing","priority":"Urgent"}'

# Bearer auth instead of X-API-Key -> 201
curl -i -X POST $EP -H "Authorization: Bearer $KEY" -H 'content-type: application/json' \
  -d '{"title":"via bearer"}'

# with assignee (email of a team-org member) -> 201 + {"assignee":{"resolved":true,...}}
curl -i -X POST $EP -H "X-API-Key: $KEY" -H 'content-type: application/json' \
  -d '{"title":"Assigned issue","assignee":"dev@rokomari.com"}'

# unknown assignee -> 201 but {"assignee":{"resolved":false,"reason":"no org member with that email"}}
curl -i -X POST $EP -H "X-API-Key: $KEY" -H 'content-type: application/json' \
  -d '{"title":"bad assignee","assignee":"nobody@rokomari.com"}'
```

### Expected failures

```bash
# no key -> 401
curl -i -X POST $EP -H 'content-type: application/json' -d '{"title":"x"}'

# wrong key -> 401
curl -i -X POST $EP -H "X-API-Key: nope" -H 'content-type: application/json' -d '{"title":"x"}'

# missing title -> 400
curl -i -X POST $EP -H "X-API-Key: $KEY" -H 'content-type: application/json' -d '{"description":"no title"}'

# bad priority -> 400
curl -i -X POST $EP -H "X-API-Key: $KEY" -H 'content-type: application/json' -d '{"title":"x","priority":"banana"}'

# invalid JSON -> 400
curl -i -X POST $EP -H "X-API-Key: $KEY" -H 'content-type: application/json' -d '{not json}'
```

## Notes

- Use `dedup_key` for anything that can fire more than once (webhook retries, repeated
  alerts) so you don't create duplicate issues.
- Operator/setup details (service account, profiles, deployment) are in
  [README.md](./README.md).

---

## Agent-Centric Requirement Draft API (approval-gated)

This flow is designed for heavier requirement inputs where an agent proposes an epic and
child tasks, but a human must approve before publishing.

### Create draft

```
POST https://vk.rokomari.io/ingest/requirements/drafts
```

Body:

```json
{
  "source": {
    "type": "pdf",
    "fingerprint": "req-checkout-v2",
    "object_keys": [
      "requirements/checkout-v2.pdf"
    ],
    "links": [
      "https://r2.example.com/requirements/checkout-v2.pdf"
    ]
  },
  "epic": {
    "title": "Checkout v2 requirements",
    "summary": "Implement revised checkout flow with fraud checks and fallback paths.",
    "acceptance_criteria": [
      "User can complete payment with saved cards.",
      "Fraud failure paths are surfaced with clear UI."
    ]
  },
  "child_tasks": [
    {
      "title": "Backend payment orchestration",
      "objective": "Add retry-safe orchestration for providers.",
      "acceptance_criteria": [
        "Retries are idempotent.",
        "Failure reasons are persisted."
      ],
      "priority": "High",
      "assignee": "dev@rokomari.com"
    }
  ],
  "generated_by": "requirements-agent"
}
```

Notes:

- `source.type`: `pdf` | `docx` | `markdown`
- `source.fingerprint`: stable key used for revision tracking
- `source.object_keys` (optional): object keys in your private R2 bucket. Ingest stores durable `r2://bucket/key` references and returns short-lived signed links for access.
- If `child_tasks` are omitted, the service derives tasks from epic acceptance criteria.
- Child task cap is enforced (`INGEST_MAX_CHILD_TASKS`, default `12`) with one follow-up backlog task when overflow exists.
- Fresh and revision drafts are both approval-gated.

Response:

- `201` with `draft_id`, `change_type` (`new|revision`), `revision_number`

### List drafts

```
GET https://vk.rokomari.io/ingest/requirements/drafts
GET https://vk.rokomari.io/ingest/requirements/drafts?status=draft
```

Response:

- `200 { "drafts": [...] }`

### Get draft (includes signed links)

```
GET https://vk.rokomari.io/ingest/requirements/drafts/{draft_id}
```

Response:

- `200` full draft payload
- `404` draft not found

### Get fresh signed source links

Use this endpoint when links may have expired.

```
GET https://vk.rokomari.io/ingest/requirements/drafts/{draft_id}/signed-source-links
GET https://vk.rokomari.io/ingest/requirements/drafts/{draft_id}/signed-source-links?ttl_secs=300
```

Response:

- `200 { "draft_id": "...", "signed_source_links": [{ "key":"...", "url":"...", "expires_in_secs":300 }] }`
- `503` when R2 signing is not configured

### Upload source file (PUT raw body)

Upload file bytes directly through ingest to private R2.

```
PUT https://vk.rokomari.io/ingest/requirements/sources/{object_key}
```

Example:

```bash
curl -X PUT "https://vk.rokomari.io/ingest/requirements/sources/requirements/checkout-v2.pdf" \
  -H "X-API-Key: $KEY" \
  -H "Content-Type: application/pdf" \
  --data-binary "@checkout-v2.pdf"
```

Response:

```json
{
  "uploaded": true,
  "object_key": "requirements/checkout-v2.pdf",
  "durable_uri": "r2://<bucket>/requirements/checkout-v2.pdf",
  "signed_get_url": "https://...X-Amz-Signature=...",
  "expires_in_secs": 300,
  "size_bytes": 123456,
  "content_type": "application/pdf"
}
```

### Upload source file (multipart form)

```
POST https://vk.rokomari.io/ingest/requirements/sources/upload
Content-Type: multipart/form-data
```

Form fields:

- `file` (required)
- `object_key` (optional)
- `prefix` (optional; used when `object_key` is not provided)

Example:

```bash
curl -X POST "https://vk.rokomari.io/ingest/requirements/sources/upload" \
  -H "X-API-Key: $KEY" \
  -F "file=@checkout-v2.pdf" \
  -F "prefix=requirements/srs"
```

Use returned `object_key` in draft payload as `source.object_keys`.

### Approve and publish

```
POST https://vk.rokomari.io/ingest/requirements/drafts/{draft_id}/approve
```

Optional body:

```json
{ "approved_by": "sakib@rokomari.com" }
```

Behavior:

- Publishes epic + child tasks to Vibe Kanban.
- If the source fingerprint already maps to an existing epic, that epic is updated and a new revision is recorded.
- Existing child tasks are reused/updated by position; extra old tasks are marked superseded.

Response:

- `200 { "approved": true, "epic_issue_id": "...", "child_issues": [...], "superseded_child_issues": [...] }`

### Reject draft

```
POST https://vk.rokomari.io/ingest/requirements/drafts/{draft_id}/reject
```

Optional body:

```json
{ "reason": "Need clearer acceptance criteria." }
```

Response:

- `200 { "rejected": true, "draft_id": "..." }`
