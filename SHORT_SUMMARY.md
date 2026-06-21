# Vibe Kanban — Short Summary

> Self-hosted, team-wide AI-assisted kanban for Rokomari Engineering.  
> Central shared state · local agent execution · zero per-seat licensing cost.

---

## 1. What It Is

**Vibe Kanban** is an open-source (Apache-2.0) platform for planning software work on a kanban board and executing it with AI coding agents (Claude Code, Cursor, Codex, Gemini CLI, and others).

The **Rokomari deployment** (`rok-vibe-kanban-team`) wraps upstream Vibe Kanban with downstream patches (Zoho OAuth, GitLab MRs, domain restrictions, ingest API) and runs it as a **Docker Compose stack** on a single VM at `https://vk.rokomari.io`.

| Layer | Responsibility |
|-------|----------------|
| **Central server** | Orgs, projects, issues, assignment, auth, real-time sync |
| **Local client** | Kanban UI, workspaces, terminals, git worktrees, AI agents |
| **Developer machine** | All code execution — agents never run on the central VM |

---

## 2. Executive Working Model

### Roles

| Role | Who | Primary actions |
|------|-----|-----------------|
| **Platform / DevOps** | Small ops team | Deploy Compose stack, manage secrets, backups, upgrades, invites |
| **Lead / Admin** | Engineering leads | Create issues, assign developers, track board, dispatch work |
| **Developer** | All engineers | Run local client, pick up assigned issues, execute agents locally |

### Day-to-day flow

```
┌─────────────────────────────────────────────────────────────────────────┐
│                         EXECUTIVE WORKING MODEL                         │
└─────────────────────────────────────────────────────────────────────────┘

  PLAN (central)                ASSIGN (central)              EXECUTE (local)
  ─────────────                 ───────────────               ───────────────

  Lead creates issue     →     Lead assigns to dev     →     Dev opens local client
  on Amaly kanban board        via UI or ingest API          (npx @rokomari/vibe-kanban)
        │                            │                              │
        ▼                            ▼                              ▼
  Issue lands in            Assignee sees issue in         Dev spins up workspace:
  "To do" on board          personal queue (real-time      branch + terminal + agent
                            sync via ElectricSQL)          + diff review + PR
        │                            │                              │
        └────────────────────────────┴──────────────────────────────┘
                                     │
                                     ▼
                            REVIEW & SHIP (local + GitHub/GitLab)
                            ─────────────────────────────────────
                            Inline comments → agent iterates → open MR/PR → merge
```

### Operating principles

1. **One source of truth** — PostgreSQL on the central VM owns all team issue state.
2. **Mandatory local client** — Every developer runs `npx @rokomari/vibe-kanban`; there is no browser-only execution path.
3. **Explicit assignment** — Work is routed to named owners, not just a shared board.
4. **Local power, central coordination** — AI agents, git, and dev servers run on the developer's machine; only metadata syncs centrally.
5. **Closed team** — Zoho OAuth + `@rokomari.com` domain restriction; invite-based org membership.
6. **Automation-ready** — External tools can create issues via `POST /ingest/issues` (optional ingest profile).

### Key artifacts

| Artifact | Purpose |
|----------|---------|
| `https://vk.rokomari.io` | Central API + web portal (auth, invites, admin) |
| `npx @rokomari/vibe-kanban` | Developer client launcher (pins version, sets API base) |
| Org **"Rokomari SE Team"** | Shared team workspace |
| Project **"Amaly"** | Default project for issues and ingest |
| `scripts/invite.sh` | Create org invitations (manual link delivery today) |
| `ingest/` sidecar | Programmatic issue creation from internal tools |

---

## 3. System Design (ASCII)

### 3.1 High-level architecture

```
                              INTERNET
                                 │
                                 │  HTTPS (:443)
                                 ▼
┌────────────────────────────────────────────────────────────────────────────┐
│                     CENTRAL VM  (Docker Compose)                           │
│                     vk.rokomari.io                                         │
│  ┌──────────────────────────────────────────────────────────────────────┐  │
│  │  CADDY  ── TLS termination (Let's Encrypt) ── reverse proxy          │  │
│  │    │                                                                 │  │
│  │    ├── /v1/*  ──────────────────────────►  REMOTE SERVER  (:8081)    │  │
│  │    ├── /ingest/*  (opt-in profile)  ───►  INGEST SIDECAR  (:8090)    │  │
│  │    └── static UI (remote-web)  ────────►  REMOTE SERVER  (/srv/static)│  │
│  └──────────────────────────────────────────────────────────────────────┘  │
│                                 │                                          │
│                    ┌────────────┼────────────┐                             │
│                    ▼            ▼            ▼                             │
│              ┌──────────┐ ┌──────────┐ ┌──────────────┐                    │
│              │ POSTGRES │ │ ELECTRIC │ │ INGEST       │                    │
│              │  :5432   │ │  :3000   │ │ (optional)   │                    │
│              │          │ │ internal │ │              │                    │
│              │ wal_level│ │ sync     │ │ service acct │                    │
│              │ =logical │ │ layer    │ │ → remote API │                    │
│              └────┬─────┘ └────┬─────┘ └──────┬───────┘                    │
│                   │            │              │                            │
│                   └────────────┴──────────────┘                            │
│                         internal Docker network                            │
└────────────────────────────────────────────────────────────────────────────┘
         ▲                    ▲                         ▲
         │                    │                         │
    OAuth callback        Real-time sync           API + static
    REST API              (proxied via remote)     (auth portal)
         │                    │                         │
         │                    │                         │
┌────────┴────────┐  ┌────────┴────────┐      ┌────────┴────────┐
│  Developer A    │  │  Developer B    │      │  Lead / Admin   │
│  ─────────────  │  │  ─────────────  │      │  ─────────────  │
│  npx client     │  │  npx client     │      │  Web portal or  │
│  local-web UI   │  │  local-web UI   │      │  local client   │
│                 │  │                 │      │                 │
│  ┌───────────┐  │  │  ┌───────────┐  │      │  Create/assign  │
│  │ Workspaces│  │  │  │ Workspaces│  │      │  issues, invites│
│  │ Terminals │  │  │  │ Terminals │  │      └─────────────────┘
│  │ AI Agents │  │  │  │ AI Agents │  │
│  │ Git trees │  │  │  │ Git trees │  │
│  └───────────┘  │  │  └───────────┘  │
│  (local machine)│  │  (local machine)│
└─────────────────┘  └─────────────────┘

         ▲
         │  POST /ingest/issues  (X-API-Key)
         │
┌────────┴────────┐
│ Internal tools  │
│ (automation)    │
└─────────────────┘
```

### 3.2 Service inventory

```
┌─────────────┬────────────────────────────────────────┬───────┬─────────┐
│ Service     │ Role                                   │ Port  │ Public? │
├─────────────┼────────────────────────────────────────┼───────┼─────────┤
│ caddy       │ TLS + reverse proxy (only entrypoint)  │ 80/443│   YES   │
│ remote      │ Auth, orgs, issues, API, migrations    │ 8081  │ via CDN │
│ electric    │ Real-time DB change streaming          │ 3000  │   NO    │
│ postgres    │ Source of truth (logical replication)  │ 5432  │   NO    │
│ electric-init│ One-shot: wait for remote /health   │   —   │   NO    │
│ ingest      │ Optional issue-ingestion sidecar       │ 8090  │ via CDN │
└─────────────┴────────────────────────────────────────┴───────┴─────────┘
```

### 3.3 Data & sync flow

```
  MUTATION PATH (create / assign / update issue)
  ───────────────────────────────────────────────

  Client ──HTTPS──► Caddy ──► Remote ──SQL──► PostgreSQL
                                  │
                                  └──► Electric reads WAL (logical replication)


  SYNC PATH (near-real-time updates to all clients)
  ─────────────────────────────────────────────────

  Client ──subscribe shape──► Remote (JWT validated)
                                  │
                                  └──proxy──► Electric ──► PostgreSQL
                                                    │
  Client ◄──────────── live issue/assignee updates ─┘


  INGEST PATH (automation → issue creation)
  ─────────────────────────────────────────

  Tool ──POST /ingest/issues──► Caddy ──► Ingest ──local auth──► Remote API
                                              │
                                              └── dedup_key, assignee resolution
```

### 3.4 Core data model (assignment-focused)

```
  organizations
       │
       ├── organization_member_metadata  (user + role: ADMIN | MEMBER)
       ├── organization_invitations
       └── projects
                │
                ├── project_statuses  (e.g. "To do", "In progress", "Done")
                └── issues
                         │
                         ├── status_id  ──► project_statuses
                         └── issue_assignees  (issue ↔ user, many-to-many)

  Personal queue  =  search_issues(assignee_user_id = me)
  Lead dispatch   =  unassigned issues + bulk assign via UI / CLI / ingest
```

### 3.5 Auth & identity

```
  Developer                    Central Remote
      │                              │
      │  "Continue with Zoho"        │
      ├─────────────────────────────►│  OAuth (GitHub / Google / Zoho)
      │                              │
      │◄────── JWT session token ────┤
      │                              │
      │  ALLOWED_EMAIL_DOMAINS       │
      │  = rokomari.com (enforced)   │
      │                              │
      │  Invite link accept          │
      ├─────────────────────────────►│  Join org → access Amaly project
```

### 3.6 Repository layout

```
  rok-vibe-kanban-team/          ← deployment & distribution repo (this repo)
  ├── vibe-kanban/               ← upstream source (git submodule, pinned tag)
  ├── patches/                   ← downstream patch stack (applied at build)
  ├── docker-compose.yml         ← central stack definition
  ├── ingest/                    ← optional issue-ingestion sidecar
  ├── rok-vibe-kanban-launcher/  ← @rokomari/vibe-kanban npm wrapper
  ├── scripts/                   ← apply-patches, invite, backup, build
  └── SYSTEM_DESIGN.md           ← full architecture reference
```

---

## 4. Technology Stack (at a glance)

| Component | Technology |
|-----------|------------|
| Remote API | Rust (Axum), SQLx |
| Frontend | React + TypeScript (local-web, remote-web, web-core) |
| Database | PostgreSQL 16 (`wal_level=logical`) |
| Real-time sync | ElectricSQL |
| TLS / proxy | Caddy (Let's Encrypt) |
| Client distribution | npm (`@rokomari/vibe-kanban`) |
| Deployment | Docker Compose on single Linux VM |
| Auth | OAuth (Zoho primary) + bootstrap local admin |

---

## 5. Related docs

| Document | Contents |
|----------|----------|
| [SYSTEM_DESIGN.md](./SYSTEM_DESIGN.md) | Full architecture, ER diagrams, design decisions |
| [HANDOFF.md](./HANDOFF.md) | Production facts, gotchas, open items |
| [DEPLOYMENT_README.md](./DEPLOYMENT_README.md) | Compose deployment runbook |
| [DEVELOPER_ONBOARDING.md](./DEVELOPER_ONBOARDING.md) | Developer setup guide |
| [ingest/README.md](./ingest/README.md) | Issue ingestion API |
