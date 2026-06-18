# Vibe Kanban — Issue Ingestion API

Create issues on the central board (`https://vk.rokomari.io`) by POSTing JSON.
Hand this file to whoever builds the calling integration.

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
