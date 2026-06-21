# PR Notes

## Summary

This PR introduces an agent-centric requirement ingestion pipeline with strict human approval, private R2 source handling, and automatic memory knowledgebase ingestion.

## Why

- Business requirement inputs are long and document-heavy (PDF/DOCX/Markdown).
- Teams need controlled publish (human-in-the-loop) before issues go live.
- Agents need retrievable historical requirement context for future work.

## What Changed

### Ingest (requirement workflow)

- Added requirement draft lifecycle:
  - create/list/get/approve/reject
- Enforced strict approval gate before publish.
- Added revision-aware publish behavior:
  - reuse/update existing child tasks
  - supersede removed old tasks
- Added private R2 signed source link support.
- Added direct source upload endpoints:
  - raw `PUT` upload
  - multipart upload

### Memory integration

- Added `POST /memory/ingest/requirements`.
- On draft approval, ingest now auto-pushes requirement content into memory.
- Memory records include epic/task content + revisioned source refs.
- Optional extraction of signed source text for markdown/txt sources.

### Reliability / bug fixes

- Replaced custom multipart parser with `busboy` to prevent boundary-token truncation/corruption in binary uploads.

## API Additions

- Ingest:
  - `POST /ingest/requirements/drafts`
  - `GET /ingest/requirements/drafts`
  - `GET /ingest/requirements/drafts/{draft_id}`
  - `POST /ingest/requirements/drafts/{draft_id}/approve`
  - `POST /ingest/requirements/drafts/{draft_id}/reject`
  - `GET /ingest/requirements/drafts/{draft_id}/signed-source-links`
  - `PUT /ingest/requirements/sources/{object_key}`
  - `POST /ingest/requirements/sources/upload`
- Memory:
  - `POST /memory/ingest/requirements`

## Config Changes

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

## Validation Performed

- Local syntax checks:
  - `node --check ingest/server.js`
  - `node --check memory/server.js`
- Focused bug repro + fix verification:
  - multipart boundary collision case reproduced
  - confirmed fixed after `busboy` migration
- End-to-end scenario:
  - upload source -> create draft -> approve -> publish issues -> auto memory ingest -> memory search hit
- Added reusable test script:
  - `scripts/e2e-requirements-memory.sh`

## Rollout Checklist

- [ ] Configure R2 private bucket env vars.
- [ ] Configure memory API key and ingest->memory URL/key.
- [ ] Deploy `ingest` + `memory` updates.
- [ ] Run `scripts/e2e-requirements-memory.sh` (or prod-adapted equivalent).
- [ ] Verify approval response includes successful `memory_ingest`.
- [ ] Verify `memory.search` returns requirement records by fingerprint/topic.
