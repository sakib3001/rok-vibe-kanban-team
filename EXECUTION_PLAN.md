# Centralized Vibe Kanban — Execution Plan

> Companion to [SYSTEM_DESIGN.md](./SYSTEM_DESIGN.md) and a revision of
> [REQUIREMENTS_AND_PLAN.md](./REQUIREMENTS_AND_PLAN.md).
>
> **Bottom line:** the requirements are fully achievable with free / open-source
> software. The published images already provide the central server, real-time sync,
> OAuth, org/role membership, per-developer assignment, and the `npx` launcher. We deploy
> them with **Docker Compose** (no Kubernetes). This is a **deploy + configure + onboard**
> effort, not a feature-build effort. Ready-to-use stack: [`DEPLOYMENT_README.md`](./DEPLOYMENT_README.md).

> **As-built status (current)**
> | Phase | State |
> |-------|-------|
> | P0 Local validation | ✅ done |
> | P1 Central deploy (`vk.rokomari.io`) | ✅ live (GHCR images, bootstrap local auth; OAuth/Harbor deferred) |
> | P1.5 Client launcher | ✅ built (`@rokomari/vibe-kanban`, Node ≥ 20, port 8154, install/uninstall) — npm publish pending |
> | P2 Org & assignment | ◑ org "Rokomari SE Team" + project "Amaly" exist; email invites deferred |
> | P7 Ingestion API | ✅ live on prod (`POST /ingest/issues`, `--profile ingest`) |
> | Open | pin prod `IMAGE_TAG`; enable OAuth; publish wrapper; P4 dispatch CLI; P5 runbooks |

---

## 0) Pre-flight Facts (read once)

- **License:** upstream is **Apache 2.0** (free, no seats/keys); distribution layer is
MIT. The team README's "BSL" reference is stale — fixing it is task **P1-T1**.
- **Maintenance reality:** upstream is community-maintained; we own a controlled fork
via the patch stack. Plan for self-sufficiency.
- **Deployment substrate: Docker Compose, remote-only** (decided — see SYSTEM_DESIGN §8).
No Kubernetes. Developers run local `npx` clients. The k8s-only browser frontend is out
of scope. Concrete artifacts live in [`DEPLOYMENT_README.md`](./DEPLOYMENT_README.md).
- **Client policy:** local `npx` is mandatory for all developers. Browser-only mode is
not an accepted default in this rollout.

### Hard prerequisites (block everything downstream)

- A Linux VM with **Docker + Docker Compose v2**
- Ports **80 and 443** open to the internet (Caddy / Let's Encrypt)
- A domain with a **single A record → the VM's public IP** (no wildcard needed)
- An OAuth app (GitHub or Google)
- *(handled by the stack, not you:)* PostgreSQL with `wal_level=logical` + the
`electric_sync` REPLICATION role — both configured automatically in
`docker-compose.yml` + `init-db/`.

---

## Phase 0 — Local Validation

**Goal:** confirm the Compose stack comes up end-to-end on a laptop/scratch VM.


| #      | Task                                                                                                                                                    | Done when                                                 |
| ------ | ------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------- |
| P0-T1  | `cp .env.example .env`; generate secrets; switch `Caddyfile` to the `http://localhost` block                                                            | `.env` filled; local block active                         |
| P0-T1a | Pick a local-auth path for localhost testing: (A) temporary OAuth app with localhost callback, or (B) temporary bootstrap local auth in dev-only `.env` | sign-in path for `http://localhost` is explicitly defined |
| P0-T2  | `docker compose up -d`; watch `logs -f remote` for successful migrations                                                                                | remote serves `/health` 200                               |
| P0-T3  | Verify sync layer: `docker compose exec electric wget -qO- http://localhost:3000/v1/health`                                                             | Electric returns OK                                       |
| P0-T4  | Point one local client at `http://localhost`, sign in, create a test issue                                                                              | issue persists (visible after `compose restart`)          |


**Deliverable:** a reproducible local stack + confirmed baseline behavior.

---

## Phase 1 — Central Server Deployment (Production)

**Goal:** a stable HTTPS central backend on one VM.


| #     | Task                                                                                                                | Done when                            |
| ----- | ------------------------------------------------------------------------------------------------------------------- | ------------------------------------ |
| P1-T1 | **Fix license reference** in `rok-vibe-kanban-team/README.md` (BSL → Apache 2.0)                                    | README states Apache 2.0             |
| P1-T2 | Provision the VM; open 80/443; create the A record `PUBLIC_DOMAIN → VM IP`                                          | DNS resolves to the VM               |
| P1-T3 | Register the OAuth app; callback `https://<PUBLIC_DOMAIN>/v1/oauth/callback/<provider>`                             | client ID/secret in hand             |
| P1-T4 | Fill production `.env` (secrets, `PUBLIC_DOMAIN`, `ACME_EMAIL`, `ALLOWED_EMAIL_DOMAINS`, OAuth); **pin image tags** | `.env` complete, tags pinned         |
| P1-T5 | Restore the production `Caddyfile` (real domain block)                                                              | Caddy config set                     |
| P1-T6 | `docker compose up -d`; confirm Caddy obtains a Let's Encrypt cert                                                  | `https://<PUBLIC_DOMAIN>/health` 200 |
| P1-T7 | Validate OAuth login + persistence across `docker compose restart`                                                  | login + persistence confirmed        |
| P1-T8 | Enable boot survival: `restart: unless-stopped` (already set) + Docker enabled on boot (or a systemd unit)          | stack returns after VM reboot        |


**Deliverable:** stable central HTTPS URL serving the team backend.

---

## Phase 1.5 — Client Standardization Gate (mandatory before team rollout)

**Goal:** enforce one local power-user client path for all developers.


Central endpoint: **`https://vk.rokomari.io`**. Client config knob: **`VK_SHARED_API_BASE`**.

> **Decision (packaging):** standard client is the **`npx` wrapper**, run natively — *not*
> a Docker dev container. Rationale: local mode exists so agents use the developer's real
> OS, runtimes, git, IDE, and AI-tool auth directly; a laptop container would add per-dev
> mount/credential setup without delivering the true zero-install browser experience
> (that requires the sysbox runtime, i.e. the excluded k8s path). Revisit only if toolchain
> drift across machines becomes a real problem.

| #       | Task                                                                                          | Done when                                                      | Status |
| ------- | --------------------------------------------------------------------------------------------- | -------------------------------------------------------------- | ------ |
| P1.5-T1 | Build the `@rokomari/vibe-kanban` wrapper that pre-sets `VK_SHARED_API_BASE=https://vk.rokomari.io` and pins the client | wrapper source exists in `rok-vibe-kanban-launcher/` | ✅ done |
| P1.5-T2 | Pin client version to the deployed remote build + document upgrade policy | client pinned to **`0.1.44-20260617110518`** (npm `latest`; bare `0.1.44` does not exist on npm); policy in launcher README. **Still pin the server `IMAGE_TAG`** (currently `latest`) and align it to this build | ◑ client pin fixed; pin+align server tag |
| P1.5-T3 | Publish to the chosen npm registry (public `@rokomari` org or internal) | `npx @rokomari/vibe-kanban` resolves for a developer | ◻ needs registry access |
| P1.5-T4 | Smoke-test from a clean machine (sign in, open assigned issue, start local agent run) | end-to-end local power-user flow succeeds | ◻ needs a clean machine |
| P1.5-T5 | One-shot installer + user-level systemd service (Node 22 via NodeSource, fixed port 8154, boot+linger) + uninstaller | `./install.sh` yields a running `--user` service at `http://127.0.0.1:8154`; `./uninstall.sh` reverses it | ✅ `install.sh` + `uninstall.sh` built |

**Deliverable:** one mandatory local client command for everyone — `npx @rokomari/vibe-kanban`.
Artifact: [`rok-vibe-kanban-launcher/`](./rok-vibe-kanban-launcher/).

---

## Phase 2 — Organization & Assignment Workflow

**Goal:** a working, documented lead→developer assignment loop.


| #     | Task                                                                                                    | Done when                    |
| ----- | ------------------------------------------------------------------------------------------------------- | ---------------------------- |
| P2-T0 | **Configure an email provider** for invitation emails (set `LOOPS_EMAIL_API_KEY`, or accept invite-link sharing) — see note below | invites deliver, OR link-sharing process documented |
| P2-T1 | Create the team **Organization** and first **Project**                                                  | org + project exist          |
| P2-T2 | Enable **allowed-email-domain** restriction; invite developers                                          | only company emails can join |
| P2-T3 | Verify roles: at least one `admin` (lead) and `member` (dev) in `organization_member_metadata`          | roles enforced               |
| P2-T4 | Validate assignment lifecycle: create issue → assign to dev → dev sees it via `assignee_user_id` filter | dev retrieves own queue      |
| P2-T5 | Validate unassigned discovery for leads (issues with no assignee)                                       | lead lists unassigned        |
| P2-T6 | **Decision gate:** is the assignee filter UX sufficient, or is a dedicated "My Issues" view needed?     | decision recorded            |


> **Email prerequisite (found in Phase 0):** invitation emails are **disabled** unless an
> email provider is configured — the remote logs `Email service not configured — skipping
> org invitation email. Set LOOPS_EMAIL_API_KEY to enable.` Two options:
> 1. Set `LOOPS_EMAIL_API_KEY` (add it to `.env` and the `remote` service env) so invites
>    send automatically, **or**
> 2. Skip email and have leads **share invite links manually** (the invitation row is still
>    created in `organization_invitations`). Document whichever you choose in the lead playbook (P5-T2).

**Deliverable:** documented, working assignment process. (If P2-T6 says a dedicated view
is required, it becomes a patch task in Phase 6.)

---

## Phase 3 — Developer Launch Standardization

**Goal:** one command, zero manual config, for every developer.


| #     | Task                                                                                                       | Done when                                   |
| ----- | ---------------------------------------------------------------------------------------------------------- | ------------------------------------------- |
| P3-T1 | Roll out wrapper usage to all developers; remove fallback docs that require manual base URL setup          | all developers use the wrapper-only flow    |
| P3-T2 | Validate 3+ developer machines on different environments (fresh laptop, existing machine, CI/devbox image) | all can run assigned work with same command |


**Deliverable:** one predictable launch command for every developer.

---

## Phase 4 — Dispatch Tooling (Lead Operations)

**Goal:** repeatable task routing.


| #     | Task                                                                                                                    | Done when              |
| ----- | ----------------------------------------------------------------------------------------------------------------------- | ---------------------- |
| P4-T1 | Build a thin lead CLI over the existing search/assign API: assign one, bulk assign, list unassigned, list a dev's queue | all four commands work |
| P4-T2 | Store admin/service auth securely (env/secret, not committed)                                                           | no secrets in git      |
| P4-T3 | Document the dispatch workflow for leads                                                                                | runbook exists         |


**Deliverable:** repeatable routing workflow.

---

## Phase 5 — Onboarding & Runbooks

**Goal:** anyone can join, dispatch, or operate from docs alone.


| #     | Task                                                                                                                    | Done when                                   |
| ----- | ----------------------------------------------------------------------------------------------------------------------- | ------------------------------------------- |
| P5-T1 | **Developer onboarding**: prerequisites, launch command, login/join org, pick assigned task, start workspace — see [DEVELOPER_ONBOARDING.md](./DEVELOPER_ONBOARDING.md) | new dev completes first task from docs only — ✅ draft written; validate with a real new dev |
| P5-T2 | **Lead playbook**: create/assign/bulk-assign, monitor progress                                                          | lead operates unaided                       |
| P5-T3 | **Operations runbook**: `compose pull/up` upgrade flow, `pg_dump` backup + restore drill, health checks, incident steps | runbook validated by a real restore drill   |
| P5-T4 | Ship container logs into existing EFK stack for retention                                                               | logs searchable centrally                   |


**Deliverable:** complete docs for developers, leads, and operators.

---

## Phase 6 — Optional Enhancements

Only after the manual workflow is stable.


| #     | Task                                                                              | Trigger                                     |
| ----- | --------------------------------------------------------------------------------- | ------------------------------------------- |
| P6-T1 | Dedicated "My Issues" frontend view (downstream patch)                            | if P2-T6 required it                        |
| P6-T2 | Auto-routing rules: round-robin / load-based / tag-based                          | manual dispatch overhead is measurable      |
| P6-T3 | Off-host automated backups (cron `pg_dump` → object storage) + restore automation | reliability needs grow                      |
| P6-T4 | Browser-mode (k8s + Helm + sysbox) for zero-install onboarding                    | if local-install friction becomes a problem |


**Deliverable:** reduced manual dispatch overhead / hardened ops.

---

## Phase 7 — Issue Ingestion API (sidecar)

**Goal:** `POST /ingest/issues` from an internal tool auto-creates an issue centrally.
**Approach (decided):** sidecar container behind Caddy; static API key; one fixed project;
default status; payload `{title, description?, priority?, dedup_key?, assignee?}`.

| #     | Task | Done when | Status |
|-------|------|-----------|--------|
| P7-T1 | Build the `ingest` sidecar (Node, no deps): API-key auth, service-account login + token refresh, default-status resolve, optional dedup | code + Dockerfile + compose profile + Caddy route | ✅ built (`ingest/`) |
| P7-T2 | One-time setup: service-account (reused `admin@rokomari.io` local-auth — already a team-org admin); `INGEST_PROJECT_ID`=Amaly (`63051e48`) | service account can create in the team project | ✅ done on prod |
| P7-T3 | Deploy + smoke test: `--profile ingest`, `curl` creates an issue; verify dedup | issue appears in the board; repeat with same `dedup_key` returns same id | ✅ verified local + **prod** (401/201/200-dedup) |
| P7-T4 | Optional `assignee` (email) → resolve via org members (`INGEST_ORG_ID`) → `POST /v1/issue_assignees`; unresolved = create unassigned + report | assigning by member email works; bad email still creates issue | ✅ verified local + **prod** |

**Deliverable:** authenticated POST endpoint that reliably creates (de-dupes, and optionally assigns) issues.

---

## Milestones & Acceptance Criteria


| Milestone           | Acceptance                                                                             |
| ------------------- | -------------------------------------------------------------------------------------- |
| M1 Central backend  | API reachable over HTTPS; OAuth + org/project flows work; data persists                |
| M2 Assignment       | Lead assigns an issue to a target dev; dev sees it in their personal queue             |
| M3 Local execution  | Dev starts a local workspace (`npx`) from an assigned issue; status reflects centrally |
| M4 Dispatch tooling | Lead bulk-assigns and lists unassigned queues from the CLI                             |
| M5 Onboarding       | A new dev joins and completes their first assigned task using documentation only       |


---

## Risk Register


| Risk                                   | Likelihood | Impact | Mitigation                                                                     |
| -------------------------------------- | ---------- | ------ | ------------------------------------------------------------------------------ |
| Upstream community-maintained / slows  | Med        | Med    | Own the patch stack + nightly tracking; pin known-good image tags              |
| Single-VM = single point of failure    | Med        | Med    | Off-host backups (P6-T3); reprovision-from-`.env`+backup runbook; VM snapshots |
| `latest` image pulls a breaking change | Med        | High   | **Pin image tags** in `.env` (P1-T4); upgrade deliberately                     |
| Electric grants only apply on fresh DB | Low        | Med    | `init-db/` runs on first init; documented re-grant in `DEPLOYMENT_README.md`   |
| "My Issues" UX gap                     | Low        | Low    | Decision gate P2-T6; small patch if needed                                     |
| Misconfigured client base URL          | Low        | Low    | npm wrapper pre-sets the API base                                              |
| Secret leakage                         | Low        | High   | git-ignored `.env`; rotate `JWT_SECRET`; least-privilege OAuth app             |
| Data loss                              | Low        | High   | Scheduled `pg_dump` + tested restore drill (P5-T3)                             |


---

## Immediate Next Steps (after approval)

1. Provision a Linux VM with Docker; open 80/443; point an A record at it.
2. Register the OAuth app (callback `https://<domain>/v1/oauth/callback/<provider>`).
3. Execute **Phase 0** locally to validate the Compose stack, then **Phase 1** on the VM.
4. Fill production `.env` with generated secrets and **pinned image tags**.

