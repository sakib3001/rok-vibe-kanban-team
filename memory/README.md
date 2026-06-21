# Organization Memory service

Small sidecar service for organization memory retrieval and ingestion.

## What it provides

- REST endpoints under `/memory/*` (API-key guarded)
- MCP stdio mode with `memory.*` tools
- Dedicated `memory-db` (pgvector) schema + migrations
- Pluggable embedding provider (`ollama` default)
- Degraded keyword fallback when embedder is unavailable

## Endpoints

- `GET /health`
- `POST /memory/search`
- `POST /memory/runbook`
- `POST /memory/recent_changes`
- `POST /memory/issue_context`
- `POST /memory/project_brief`
- `POST /memory/ingest/docs`
- `POST /memory/ingest/issues`
- `POST /memory/ingest/requirements`
- `POST /memory/reembed`  (`{"only_missing":true}` to heal null-embedding rows)

### Requirement ingestion payload (summary)

`POST /memory/ingest/requirements` expects:

- `org_id`, `project_id?`
- `source_fingerprint`, `revision_number`, `source_type`
- `epic` (`title`, `summary`, `acceptance_criteria[]`)
- `child_tasks[]`
- `source_links[]`, `signed_source_links[]` (for optional markdown/txt source extraction)
- `published` (epic/child issue ids)
- `POST /memory/retention`  (soft-delete noisy types past window; purge old soft-deletes)

## MCP mode

Run as stdio MCP server:

```bash
node server.js --mcp-stdio
```

Tools:

- `memory.search`
- `memory.recent_changes`
- `memory.runbook`
- `memory.issue_context`
- `memory.project_brief`

## Notes

- Deterministic issue/project data is read live from the Vibe Kanban remote API.
- Embedded memory data is stored only in `memory-db`.
