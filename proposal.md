# Organization Memory Proposal (for Vibe Kanban + AI Agents)

## 1) Goal

Build an organization-specific memory layer so AI agents (and humans) can retain and reuse day-to-day knowledge:

- issue history and decisions
- recurring incidents and resolutions
- team standards and runbooks
- member/project context
- activity timeline and "what changed yesterday"

This should work with your current deployment model:

- central Vibe Kanban backend on Docker Compose
- local agent execution on developer machines
- optional ingest API and dispatch CLI

---

## 2) Business understanding

### Current operating reality

- Vibe Kanban is the central source of truth for issues and assignment.
- Developers execute agents/workspaces locally, but team state is centralized.
- Work knowledge is currently fragmented across issue threads, docs, terminals, and chat.
- Leads need fast visibility for dispatching and prioritization, while developers need clear daily context.

### Business problems to solve

1. **Knowledge loss**
   - repeated incidents and decisions are rediscovered instead of reused
2. **Slow onboarding**
   - new members lack a reliable "how we work here" context layer
3. **Inconsistent execution**
   - different people solve similar problems differently without shared memory
4. **Low traceability**
   - hard to answer "why did we do this?" from historical context
5. **Agent context gaps**
   - AI agents lack organization-specific historical memory and may produce generic answers

### Business outcomes expected

- Faster decision-making and issue resolution
- Better assignment quality (right task -> right person)
- Reduced repetitive work and duplicate investigations
- Improved onboarding speed and consistency
- Higher-quality agent output with citations to org history

---

## 3) Requirements and feature set

### Functional requirements

1. **Org-scoped isolation**
   - strict separation by organization and optionally by project/team
2. **Hybrid memory ingestion**
   - structured events (issues/assignees/status) + unstructured knowledge (runbooks/notes/postmortems)
3. **Agent-friendly retrieval**
   - API + MCP-compatible access pattern for tools/agents
4. **Human-friendly knowledge access**
   - searchable memory timeline, project brief, issue context, and runbook lookup
5. **Traceable memory records**
   - every retrieved memory must include source reference and timestamp
6. **Actionable context views**
   - daily digest, recent change summary, and "what changed since X" query support

### Non-functional requirements

1. **Open-source + self-hosted**
   - no mandatory proprietary dependency
2. **Auditability**
   - source links, actor metadata, and immutable write log
3. **Security and governance**
   - role-based visibility + redaction of secrets/sensitive values
4. **Low ops overhead**
   - fit existing Compose-first operations model
5. **Performance**
   - retrieval fast enough for interactive agent calls (<1-2 seconds for normal queries)

### Feature set (MVP -> advanced)

#### MVP features

- Ingest issue lifecycle events into memory
- Ingest dispatch actions and runbook docs
- Semantic search with citations
- `issue_context` and `project_brief` retrieval endpoints
- Org/project filters in all memory queries

#### Phase-2 features

- Daily memory digest (per org/project)
- Incident + decision templates
- Memory quality scoring and stale-memory flags
- Optional chat notifications (Discord) for major memory events

#### Advanced features

- Auto-summarized weekly retrospectives
- Recommendation engine ("similar past issues and resolutions")
- Policy memory checks before agent action (safety/rules-aware planning)

---

## 4) Tooling options (open-source shortlist)

## A) Mem0 OSS (likely what you meant by "memplace")

**Best when:**
- you want memory primitives specifically for AI agent interactions (user/session/agent memory)

**Pros**
- memory-focused abstractions out of the box
- designed around retrieval and persistence for agents

**Cons**
- less "team wiki" style content management
- often needs additional pieces for governance and long-lived docs

---

## B) Zep (open-source memory store)

**Best when:**
- you want long-term memory and conversation context with better temporal/semantic retrieval

**Pros**
- strong memory/search capabilities for chat/agents
- good fit for "what happened recently + related history" use cases

**Cons**
- another standalone service to operate
- still needs ingestion and policy layers around it

---

## C) "Postgres + pgvector + lightweight memory service" (recommended baseline)

**Best when:**
- you want simplest ops and strongest control, integrated with your existing stack

**Pros**
- minimal new infrastructure
- easy org/project scoping via SQL
- easier backup/restore/audit with existing Postgres discipline

**Cons**
- you build more application logic yourself
- fewer out-of-the-box memory features than specialized tools

---

## D) Memos (team knowledge base) + vector memory backend (hybrid)

**Best when:**
- you want both a human-friendly org notebook and machine retrieval

**Pros**
- great for curated human notes, SOPs, decisions
- can complement event-derived memory

**Cons**
- requires synchronization strategy between note KB and agent memory index

---

## 5) Recommended architecture for your setup

Use a **two-layer memory model**:

1. **Operational Memory (auto-ingested)**
   - from Vibe Kanban and ingest/dispatch activity
2. **Curated Knowledge Memory (human-authored)**
   - runbooks, decisions, postmortems, team conventions

### Recommended stack

- **Storage**: PostgreSQL + pgvector (same host/cluster pattern as existing stack)
- **Memory service**: small internal service (`memory-api`) with org/project scoping
- **Ingestion worker**: consumes activity events and writes memory records
- **Retrieval API**: semantic + structured retrieval endpoints
- **Optional UX**: Memos for human KB editing, synced into memory records

Why this is best here:
- matches your Compose-first operations
- avoids immediate dependency on extra heavy infrastructure
- supports both deterministic queries and semantic context for agents

---

## 6) Memory context model (what to store)

Every memory record should include:

- `id`
- `org_id`
- `project_id` (nullable for org-global)
- `source_type` (`issue_event`, `dispatch`, `runbook`, `incident`, `decision`, `note`)
- `source_ref` (issue ID / doc path / URL)
- `title`
- `summary`
- `content` (raw text/markdown)
- `tags[]`
- `actors[]` (user IDs/emails)
- `created_at`
- `updated_at`
- `confidence` (0-1)
- `embedding` (vector)
- `visibility` (`org`, `project`, `team`, `private`)

### Core context categories

1. **Issue Lifecycle Memory**
   - created, assigned, status transitions, blockers, resolution notes
2. **Dispatch Memory**
   - assignment patterns, load balancing decisions, escalations
3. **Operational Incident Memory**
   - production incidents, root cause, mitigation, owner, follow-up
4. **Runbook Memory**
   - deploy, rollback, restore, invite flow, ingest troubleshooting
5. **Decision Log Memory**
   - architecture decisions and rationale ("why we chose X")
6. **Team Norms Memory**
   - coding standards, review rules, communication conventions
7. **Domain Memory**
   - project-specific business rules and terms

---

## 7) Ingestion plan (day-to-day activity -> memory)

## Event sources

- Vibe Kanban API events:
  - issue create/update/status change
  - assignee change
  - comments
- `ingest` API events:
  - external issue creation payloads
- dispatch CLI actions:
  - manual assign/bulk assign operations
- curated docs:
  - markdown docs at repo root (`*_README`, runbooks, plans)

## Ingestion pipeline

1. Capture event
2. Normalize to memory schema
3. Generate summary + tags
4. Embed content
5. Upsert memory record
6. Link to source (`source_ref`)

---

## 8) Retrieval plan (how agents will use it)

Expose these retrieval endpoints:

- `search_memories(org_id, query, filters...)`
- `get_issue_memory(issue_id)`
- `get_project_brief(project_id)`
- `get_recent_changes(org_id, since)`
- `get_runbook(topic, project_id?)`

And optional MCP tool wrappers:

- `memory.search`
- `memory.issue_context`
- `memory.project_brief`
- `memory.runbook`

For each response, return:
- ranked results
- short summary
- citations (`source_ref`, timestamp)

---

## 9) Access control and governance

- enforce `org_id` in every query path
- add project-level visibility checks for restricted projects
- redact secrets/tokens during ingestion
- keep immutable audit for memory writes
- support retention policy:
  - short-term noisy events (e.g., 90 days)
  - long-lived curated memory (indefinite)

---

## 10) Implementation roadmap

## Phase 1 (MVP, 1-2 weeks)

- Create memory schema and `memory-api`
- Add ingestion for:
  - issue created
  - assigned/unassigned
  - status changed
- Add semantic search + source-linked results
- Add one agent tool: `memory.search`

## Phase 2 (2-3 weeks)

- Add runbook/document ingestion from repo markdown files
- Add dispatch/ingest action ingestion
- Add `project_brief` and `issue_context` tool endpoints
- Add confidence/ranking tuning

## Phase 3 (ongoing)

- Add incident/decision templates and automated summarization
- Add "daily digest" memory compaction
- Add stale memory cleanup and quality scoring

---

## 11) Suggested Compose services

Minimum:

- `memory-api` (new service)
- `postgres` (reuse existing; enable pgvector if not already)

Optional:

- `memos` (human-facing KB UI)
- `memory-worker` (if separated from API process)

---

## 12) Example memory records

### Example A: Issue status change

- `source_type`: `issue_event`
- `title`: `AM-245 moved To do -> In progress`
- `summary`: `Sakib started AM-245 after ingest alert linked root cause to timeout handling.`
- `tags`: `["project:amaly","status-change","priority:high"]`

### Example B: Deploy runbook update

- `source_type`: `runbook`
- `title`: `Deploy script switched from Helm to Compose`
- `summary`: `scripts/deploy.sh now does backup + compose rollout + health checks.`
- `source_ref`: `scripts/deploy.sh`

---

## 13) Risks and mitigations

- **Noise overload** -> start with 3-4 high-signal event types only
- **Hallucinated memory** -> always require citations in agent responses
- **Cross-org leakage** -> hard org scoping in schema + API
- **Operational drift** -> nightly memory consistency checks (source exists, links valid)

---

## 14) Final recommendation

Start with **Postgres + pgvector + lightweight memory-api** as your core memory layer.

If you later want richer agent-native memory features, add **Mem0 OSS or Zep** as a specialized memory backend behind the same `memory-api` interface (so your agents do not need integration rewrites).

This gives:
- fastest path to value
- low ops complexity
- strong organization control
- future extensibility

