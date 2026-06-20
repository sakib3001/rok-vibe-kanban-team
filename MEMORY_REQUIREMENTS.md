# Organization Memory Layer — Finalized Requirements

> Supersedes [`proposal.md`](./proposal.md) (kept for history). This is the agreed,
> build-ready spec, adapted to the live stack: Vibe Kanban on Docker Compose
> (`postgres` + `remote` + `electric` + `caddy` + `ingest`), single VM, local MCP-capable
> AI agents, Compose-first ops.

## 1. Finalized decisions (the "what" is settled)

| # | Decision | Rationale |
|---|----------|-----------|
| D1 | **Baseline = pgvector + a small memory service.** Mem0/Zep/Memos deferred behind the same interface. | Lowest ops, strongest org control, fits the stack. |
| D2 | **Dedicated `memory-db` (pgvector), NOT the Vibe Kanban DB.** | The app DB is `postgres:16-alpine` (no pgvector) owned by VK migrations + ElectricSQL replication. Isolate to avoid coupling/backup/lifecycle risk. |
| D3 | **Query-don't-copy for structured data.** Deterministic context is read **live** from the Vibe Kanban API/DB; only **unstructured** knowledge is embedded into `memory-db`. | Eliminates sync-drift + noise-overload risks; structured state is always fresh. |
| D4 | **MCP-first retrieval.** Primary surface is an MCP server for local agents (next to the existing `vibe-kanban-mcp`); REST is the thin secondary surface. | Agents are local + MCP-capable; this is the real integration path. |
| D5 | **Pluggable embedding provider** (see §3). Default = **self-hosted** (`ollama` / `nomic-embed-text`); hosted providers swappable via config. | Honors the "no mandatory proprietary dependency" NFR while allowing higher-quality hosted models. |
| D6 | **Opt-in Compose profile `memory`**, built in the `ingest`-sidecar style (small, dependency-light, service-account auth). | Additive; can never jeopardize the central server. |
| D7 | **Ingestion v1 = curated repo docs + VK resolution/comment text** (scheduled/manual). CDC via the existing logical-replication slot is Phase 2. | Fastest value; no event pipeline to operate on day one. |

---

## 2. Architecture

```
profile: memory   (docker compose --profile memory up -d)

  memory-db    pgvector/pgvector:pg16        # dedicated; own volume + backup
  embedder     pluggable (default: ollama)   # self-hosted by default; see §3
  memory       small Node service            # MCP + thin REST, service-account auth
               ├── deterministic tools  ─────────────► Vibe Kanban remote API (live, no copy)
               └── semantic tools  ─────────────────► memory-db (pgvector) + embedder

  caddy: route  /memory/*  → memory:PORT   (REST; opt-in)
  agents: add the memory MCP server to local agent config (stdio or SSE)
```

- `memory` reuses the service account (`admin@rokomari.io` local auth) to read the VK API.
- Everything is org-scoped (`org_id` enforced in every query path), single-org today, multi-org-ready.

---

## 3. Pluggable embedding provider (REQUIRED design)

The memory service MUST treat embeddings as a **swappable provider** behind one interface.
No provider is hard-wired; selection is config-only; switching providers requires no code
change (only a re-embed — see §3.4).

### 3.1 Provider interface
A provider is any adapter implementing:
```
embed(texts: string[]) -> Promise<number[][]>   // one vector per input, fixed dimension
name: string                                     // e.g. "ollama:nomic-embed-text"
dimensions: number                               // vector size this provider emits
```
The service selects the adapter at startup from `EMBED_PROVIDER` and validates that
`dimensions` matches the `memory-db` vector column (fail fast on mismatch).

### 3.2 Built-in adapters (config-selectable)
| `EMBED_PROVIDER` | Type | Default model | Dim | Notes |
|------------------|------|---------------|-----|-------|
| `ollama` *(default)* | self-hosted | `nomic-embed-text` | 768 | CPU-fine at low volume; honors NFR |
| `openai` | hosted | `text-embedding-3-small` | 1536 | needs `EMBED_API_KEY` |
| `voyage` | hosted | `voyage-3` | 1024 | needs `EMBED_API_KEY` |
| `cohere` | hosted | `embed-english-v3.0` | 1024 | needs `EMBED_API_KEY` |
| `http` | self/any | — | `EMBED_DIMENSIONS` | generic adapter for ANY service implementing §3.3 |

### 3.3 Generic `http` adapter contract (so anything can be plugged)
```
POST {EMBED_BASE_URL}/embed
  body:   { "input": ["text a", "text b"], "model": "<EMBED_MODEL>" }
  return: { "embeddings": [[...], [...]], "model": "...", "dimensions": N }
```
Any internal/3rd-party embedding service that speaks this shape works with zero code change.

### 3.4 Dimension & switching policy
- One active provider per deployment; the `embedding` column is `vector(EMBED_DIMENSIONS)`.
- Each row stores `embed_model` + `embed_dim` (provenance). Queries embed the query with the
  **same** active provider, so search space is consistent.
- **Switching provider/model** ⇒ run `memory reembed` (re-embeds all rows with the new
  provider; rebuilds the index). Documented one-command operation; no schema migration if dim
  unchanged, otherwise an `ALTER`/new column + reindex.

### 3.5 Resilience
- Embedder unavailable → **ingestion** retries with backoff (records queued, not lost);
  **retrieval** degrades gracefully to Postgres full-text keyword search (`tsvector`) with a
  `degraded:true` flag in the response, so agents still get cited results.

### 3.6 Config (env)
```
EMBED_PROVIDER=ollama
EMBED_BASE_URL=http://embedder:11434     # ollama default; or your http service
EMBED_MODEL=nomic-embed-text
EMBED_DIMENSIONS=768
EMBED_API_KEY=                            # only for hosted providers
EMBED_BATCH=32                            # batch size for ingestion
```

---

## 4. Data model (`memory-db`)

`memory_records`:
| column | type | notes |
|--------|------|-------|
| `id` | uuid pk | |
| `org_id` | uuid, not null, **indexed** | hard scope |
| `project_id` | uuid, null | null = org-global |
| `source_type` | enum | `runbook` \| `decision` \| `incident` \| `note` \| `issue_note` \| `dispatch` |
| `source_ref` | text | doc path / VK issue id / URL — **required** (citations) |
| `title` | text | |
| `summary` | text | short, for ranked result display |
| `content` | text | raw markdown/text (redacted) |
| `tags` | text[] | |
| `actors` | text[] | user ids/emails |
| `visibility` | enum | `org` \| `project` \| `team` \| `private` |
| `confidence` | real | 0–1 |
| `embed_model` | text | provenance |
| `embed_dim` | int | provenance |
| `embedding` | `vector(EMBED_DIMENSIONS)` | ivfflat/hnsw index |
| `content_tsv` | tsvector (generated) | keyword fallback (§3.5) |
| `created_at` / `updated_at` | timestamptz | |
| `deleted_at` | timestamptz, null | soft-delete + retention |

`memory_audit` (immutable): `id, ts, actor, action(write/update/delete/reembed), record_id, source_ref`.

Structured/operational data is **not** stored here — it's read live from Vibe Kanban (D3).

---

## 5. Retrieval contract

Split by determinism. Every response includes **citations** (`source_ref` + timestamp) and is org-scoped.

### 5.1 Semantic (→ memory-db + embedder)
- `memory.search(query, {org_id, project_id?, source_type?, top_k?})` → ranked records + summaries + citations
- `memory.runbook(topic, {project_id?})` → best-matching runbook(s)

### 5.2 Deterministic (→ Vibe Kanban API, live)
- `memory.issue_context(issue_id)` → issue + status history + assignees + comments (from VK)
- `memory.project_brief(project_id)` → project + open/assigned counts + recent activity
- `memory.recent_changes(org_id, since)` → issues/assignments changed since timestamp

### 5.3 Surfaces
- **MCP server** (primary): tools above, namespaced `memory.*`, stdio + SSE; drop into each
  dev's agent MCP config beside `vibe-kanban-mcp`.
- **REST** (secondary, via Caddy `/memory/*`): same handlers, API-key guarded like `ingest`.

---

## 6. Ingestion

**v1 (scheduled or `memory ingest` command):**
- Curated docs: repo markdown (`*_README.md`, `SYSTEM_DESIGN.md`, `HANDOFF.md`, `GO_LIVE.md`,
  runbooks, decision logs) → chunk → redact → embed → upsert (keyed by `source_ref` + content hash).
- VK knowledge text: issue resolution notes / comments for closed issues → `issue_note` records.
- Idempotent: re-running updates changed docs only (content-hash compare).

**Phase 2 (CDC):** consume a Postgres **logical-replication slot** (the stack already runs
`wal_level=logical` with an `electric_sync` publication) for near-real-time `dispatch`/`incident`
memory + daily digests. Not required for MVP.

**Always:** secret/token redaction on the way in (regex + entropy heuristics).

---

## 7. Governance & security

- **Org isolation:** `org_id` is a mandatory predicate in every read/write; enforced in the
  service, not just by callers.
- **Visibility:** project/team/private checks layered on top of org scope.
- **Redaction:** strip secrets/tokens at ingest; never store raw credentials.
- **Audit:** immutable `memory_audit` for every write/update/delete/reembed.
- **Retention:** noisy `dispatch`/`issue_note` ≈ 90 days; curated (`runbook`/`decision`)
  indefinite; enforced by a retention job (soft-delete then purge).
- **Auth:** REST API-key (same pattern as `ingest`); MCP runs locally under the developer's
  own session.

---

## 8. Non-functional requirements (finalized)

- **Open-source / self-hostable**: default path uses only OSS (pgvector + Ollama + Node); no
  mandatory proprietary dependency (hosted embedders are opt-in).
- **Performance**: semantic query p95 < 1.5 s at MVP volume; deterministic tools bounded by VK API.
- **Ops**: one opt-in profile; `memory-db` included in the backup routine from day one.
- **Footprint**: must coexist on the single VM without starving the central server (cap
  embedder CPU/mem; small models only on CPU).
- **Extensibility**: provider + storage behind interfaces so Mem0/Zep can later sit behind the
  same MCP/REST contract with no agent-side change.

---

## 9. MVP scope & phases

**MVP (1–2 wk):** `memory-db` + `embedder` (ollama default) + `memory` service; ingest repo
docs (redacted, cited); MCP tools `memory.search` + `memory.recent_changes`; wire to one dev's
agent; `memory-db` in backups.

**Phase 2 (2–3 wk):** `issue_context` + `project_brief` + `runbook`; VK issue-note ingestion;
ranking/confidence tuning; REST under `/memory/*`.

**Phase 3 (ongoing):** CDC via logical-replication slot; daily digest; incident/decision
templates; stale-memory scoring; optional Memos KB UI synced into records; optional Mem0/Zep
behind the interface.

---

## 10. Acceptance criteria

- **A1** `docker compose --profile memory up -d` brings up `memory-db` + `embedder` + `memory`
  with **zero** change/risk to the running central stack.
- **A2** `EMBED_PROVIDER` can be switched (e.g. `ollama` → `http`/hosted) by config + one
  `memory reembed`, with no code change.
- **A3** `memory.search` returns org-scoped, **cited** results; cross-org queries return nothing.
- **A4** With the embedder stopped, `memory.search` still returns keyword results flagged
  `degraded:true` (resilience).
- **A5** Repo runbooks are retrievable by an agent with a working `source_ref` citation;
  secrets are redacted in stored content.
- **A6** `memory.recent_changes` reflects live Vibe Kanban state (no stale copy).

---

## 11. Decisions still needed from you

1. **Embedder default confirm:** self-hosted `ollama:nomic-embed-text` (recommended, honors
   NFR) vs a hosted default (`openai`/`voyage`). Pluggable either way — this only sets the default.
2. **VM headroom for a CPU embedder** on the shy single VM, or run the embedder elsewhere
   (the `http` adapter supports a remote embedder).
3. **Scope of VK text ingestion** (all comments vs only closed-issue resolution notes) — affects
   noise/volume.
