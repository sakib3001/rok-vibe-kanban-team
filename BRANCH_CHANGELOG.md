# Branch Changelog: `vk/d343-create-an-short`

Base branch: `v3`  
Scope: Agent-centric requirement ingestion, private R2 source handling, and memory knowledgebase integration.

## Highlights

- Added an executive short summary document for the project: `SHORT_SUMMARY.md`.
- Introduced an approval-gated requirement workflow in `ingest`:
  - create/list/get/reject/approve requirement drafts
  - strict human approval before publish
  - epic + child task publishing model
- Added revision-aware publish behavior:
  - reuses and updates existing child tasks on newer revisions
  - marks extra old child tasks as superseded instead of deleting
- Added private R2 signed URL support for requirement sources:
  - durable `r2://...` references stored
  - short-lived signed links generated on demand
- Added direct source upload APIs in ingest:
  - raw `PUT` upload path
  - multipart upload endpoint
- Added automatic memory ingestion on approved requirements:
  - new `POST /memory/ingest/requirements`
  - ingest approval now pushes requirement content into memory
  - requirement content becomes searchable via `memory.search`
- Added end-to-end local validation script:
  - `scripts/e2e-requirements-memory.sh`

## API Additions

### Ingest service

- `POST /ingest/requirements/drafts`
- `GET /ingest/requirements/drafts`
- `GET /ingest/requirements/drafts/{draft_id}`
- `POST /ingest/requirements/drafts/{draft_id}/approve`
- `POST /ingest/requirements/drafts/{draft_id}/reject`
- `GET /ingest/requirements/drafts/{draft_id}/signed-source-links`
- `PUT /ingest/requirements/sources/{object_key}`
- `POST /ingest/requirements/sources/upload`

### Memory service

- `POST /memory/ingest/requirements`

## Configuration Changes

Added/updated env support in stack/docs:

- `R2_ACCESS_KEY_ID`
- `R2_SECRET_ACCESS_KEY`
- `R2_REVIEW_ENDPOINT`
- `R2_REVIEW_BUCKET`
- `R2_REGION`
- `R2_PRESIGN_EXPIRY_SECS`
- `INGEST_UPLOAD_MAX_MB`
- `INGEST_REQUIREMENTS_FILE`
- `INGEST_MAX_BODY_KB`
- `INGEST_MAX_CHILD_TASKS`
- `INGEST_MEMORY_URL`
- `INGEST_MEMORY_API_KEY`

## Dependency Changes

- `ingest/package.json`: added `busboy` for robust multipart parsing.
- `ingest/package-lock.json`: generated/updated lockfile.

## Bug Fixes

- Fixed multipart upload truncation/corruption risk in ingest when file content contained boundary-like tokens.
  - Replaced custom multipart parser with streaming `busboy` parser.

## Test Coverage Added

- Scripted local E2E flow (`scripts/e2e-requirements-memory.sh`) validates:
  - source upload to private R2 mock
  - draft creation and approval
  - issue publish behavior
  - auto memory ingestion
  - semantic memory search hit for ingested requirement data

## Changed Files (vs `v3`)

- `.env.example`
- `SHORT_SUMMARY.md`
- `docker-compose.yml`
- `ingest/API.md`
- `ingest/README.md`
- `ingest/package-lock.json`
- `ingest/package.json`
- `ingest/server.js`
- `memory/README.md`
- `memory/server.js`
- `scripts/e2e-requirements-memory.sh`
