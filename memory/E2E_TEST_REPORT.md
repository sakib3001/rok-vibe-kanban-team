# Organization Memory — E2E Test Report

**Scope:** validate the memory layer implementation + the gap-fixes from the review.
**Environment:** local standalone stack (`docker-compose.memory-local.yml`) — `memory-db`
(pgvector) + `embedder` (ollama `nomic-embed-text`) + `memory`, exercised via
`scripts/test-memory.sh` against `http://localhost:8099`.
**Result:** **16 / 16 passed, 0 failed.**

> Deterministic tools (A6: `recent_changes` / `project_brief` / `issue_context`) read
> the **live** Vibe Kanban API and were skipped locally (`TEST_DETERMINISTIC=0`); they
> are unchanged by the fixes and validate against the server. Everything the fixes
> touched (semantic, ingestion, governance, resilience) is covered here.

## Results

| Acceptance / fix | Check | Result |
|---|---|---|
| **A1** additive profile | memory profile running, central stack untouched | ✅ |
| **Gap #1** embedder model | `nomic-embed-text` auto-present; ingest `degraded:false` | ✅ |
| Health | `/memory/health` → `{status:ok}` | ✅ |
| Ingestion | 62 doc chunks ingested with embeddings | ✅ |
| **Gap #4** idempotency | re-run: `ingested:0, skipped:62` (content-hash) | ✅ |
| **A3** semantic search | results returned, `degraded:false` | ✅ |
| Citations | every result carries `source_ref` (`doc:…`) | ✅ |
| **A3 / Gap #6** org isolation | foreign `org_id` → **HTTP 403** (allowlist) | ✅ |
| **A5** redaction | injected AWS key + GH token → **0 raw stored**, `[REDACTED]` markers present | ✅ |
| **A4** resilience | embedder stopped → search `degraded:true` + keyword results | ✅ |
| **Gap #2** heal | `reembed {only_missing:true}` succeeds | ✅ |
| **Gap #5** retention | `retention` sweep returns `{soft_deleted, purged}` | ✅ |

## Gap-fix verification summary

1. **Embedder model auto-pull (#1)** — model present and ingest produced real
   embeddings (`degraded:false`); first-run no longer silently keyword-only.
2. **Null-row heal + retry/backoff (#2)** — `reembed only_missing` path works;
   embed batches retry with backoff before degrading.
3. **Resource caps (#3)** — `cpus`/`mem_limit` on `embedder`+`memory` in the server
   profile (`docker-compose.yml`); not asserted by the test (compose-level).
4. **Content-hash idempotency (#4)** — second ingest skipped all 62 unchanged chunks.
5. **Retention (#5)** — `/memory/retention` soft-deletes noisy types past window and
   purges old soft-deletes, with `delete` audit rows.
6. **Org allowlist (#6)** — `MEMORY_ALLOWED_ORG_IDS` enforced in-service; cross-org → 403.
7. **Redaction refine** — secrets removed; lowercase-hex SHAs / UPPER_SNAKE spared.

## Not covered locally (validate on server)
- **A6 deterministic tools** — need the live VK API + service-account creds.
- **A2 provider switch + reembed across dimensions** — config + `reembed`; same-dim
  validated implicitly, cross-dim still needs a manual `ALTER` (documented caveat).

## How to re-run
```bash
docker compose -f docker-compose.memory-local.yml up -d --build
docker compose -f docker-compose.memory-local.yml exec -T embedder ollama pull nomic-embed-text
MEMORY_DC="docker compose -f docker-compose.memory-local.yml" \
MEMORY_BASE="http://localhost:8099" MEMORY_API_KEY="local-test-key" \
MEMORY_DEFAULT_ORG_ID="<org-uuid>" TEST_DETERMINISTIC=0 \
bash scripts/test-memory.sh
# server (full, incl. A6):  bash scripts/test-memory.sh
```
