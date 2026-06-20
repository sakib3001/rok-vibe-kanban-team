# Vibe Kanban — Rokomari Deployment: Agent Handoff / Full Context

Single source of truth for picking up this project. Read this first.

---

## 0. Recent changes completed (2026-06-18)

- **Repo flattened to one root:** removed the extra `docker/` wrapper directory; deployment
  artifacts now live at project root (`docker-compose.yml`, `build.sh`, `Caddyfile*`,
  `.env*`, `ingest/`, `init-db/`, `sql/`, `scripts/{invite,backup}.sh`).
- **Docs/path refresh completed:** deployment guide is now `DEPLOYMENT_README.md`; all docs
  were updated to use root-level paths.
- **Relay removed from deployment:** no `relay` service in compose, no relay env vars in
  `.env.example` / `.env.production.example`, and no relay ops instructions in docs.
- **Build + config validation completed:** `docker compose config` passes; `./build.sh`
  succeeds; production-style build also succeeds (`docker compose --env-file .env.production.example build remote`).

---

## 1. Goal & model

Self-hosted, team-wide **Vibe Kanban** for Rokomari's engineering team, built entirely on
free/open-source software (upstream is **Apache-2.0**).

- **Deployment: Docker Compose, remote-only** (NOT Kubernetes/Helm — that path was
  explicitly dropped). One central VM runs the backend; developers run a **local `npx`
  client**. Browser/code-server "frontend pod" mode was excluded (needs the sysbox runtime).
- **Central server:** `https://vk.rokomari.io`.
- Plus a custom **issue-ingestion API** and an **invite flow**.

Companion docs (project root): `REQUIREMENTS_AND_PLAN.md`, `SYSTEM_DESIGN.md`,
`EXECUTION_PLAN.md`, `DEVELOPER_ONBOARDING.md`. Deployment docs:
`DEPLOYMENT_README.md`, `GO_LIVE.md`, `ingest/README.md`, `ingest/API.md`,
`rok-vibe-kanban-launcher/README.md`.

---

## 2. Production facts

| Item | Value |
|------|-------|
| SSH | `ssh -i ~/.ssh/dev ubuntu@103.228.38.106` (host `server02`, Ubuntu, Docker no-sudo, passwordless sudo) |
| Domain | `https://vk.rokomari.io` (Cloudflare in front of the VM) |
| Compose dir on VM | `/home/ubuntu/vibe-kanban/` (**not** a git repo there — just deployed files) |
| Stack | `postgres` + `remote` (:8081) + `electric` (:3000 internal) + `caddy` (:80/:443); optional `ingest` profile |
| Remote image | `ghcr.io/iamriajul/vibe-kanban-team-remote`, pinned `IMAGE_TAG=0.1.44-20260617110518` |
| Team org | **"Rokomari SE Team"** = `a6a82264-9129-4537-aae4-63b51181b243` |
| Team project | **"Amaly"** = `63051e48-a41b-4242-8c67-138b24e7114a`, default status "To do" |
| Users | `admin@rokomari.io` (`d0b4a3d3-…`, bootstrap + **service account**, team admin) · `sakib@rokomari.com` (`798c8da6-…`, Zoho, team admin) |
| Auth | **Zoho OAuth** (DC `.com`, `accounts.zoho.com`) + bootstrap **local auth** (`admin@rokomari.io`). `ALLOWED_EMAIL_DOMAINS=rokomari.com` |
| Secrets | All in VM `/home/ubuntu/vibe-kanban/.env` (DB pw, JWT, Zoho client id/secret, INGEST_API_KEY, local-auth pw). **Not** in git. |

**⚠️ The patched frontend is currently served via a STOPGAP override on prod:**
`docker-compose.override.yml` mounts `./remote-web-dist` (a locally-built `remote-web`
bundle) over the container's `/srv/static`. This is because the GHCR image
(`0.1.44-20260617110518`) does **not** contain patches 0040/0041 (Zoho buttons + invite
redirect). The clean end-state is to build a remote image **with** those patches and pin
it, then delete the override + `remote-web-dist`. See §6.

---

## 3. Features delivered (all live on prod)

### Issue-ingestion API (`ingest/`, `--profile ingest`)
- `POST https://vk.rokomari.io/ingest/issues`, header `X-API-Key: <INGEST_API_KEY>`.
- Body: `{title (req), description?, priority? (Urgent|High|Medium|Low), dedup_key?, assignee? (team-member email)}`.
- Creates issues in **Amaly**, default status "To do". `dedup_key` = idempotent. `assignee`
  resolved against team-org members (needs `INGEST_ORG_ID=a6a82264…`); unknown email → issue
  still created, `assignee.resolved:false`.
- Node sidecar (no deps) that logs in as the service account (local auth) and calls `/v1/issues`.
  Caddy routes `/ingest/*` → `ingest:8090`. Caller ref: `ingest/API.md`.

### Invite flow
- Create: `scripts/invite.sh <email> [member|admin]` (run on the VM; reads admin
  creds + `INGEST_ORG_ID` from `.env`). Prints `https://vk.rokomari.io/invitations/<token>/accept`.
- Accept: open link → InvitationPage → **Continue with Zoho** → joins team org → "Go to your
  workspace" (→ `/`). Then visible on Amaly + assignable (UI + ingest API).
- **Loops email is NOT configured** → delivery is manual link sharing. To enable email: set
  `LOOPS_EMAIL_API_KEY` + `LOOPS_INVITE_TEMPLATE_ID` (the baked-in default template IDs are
  the upstream author's and won't work — must create your own in Loops). Templates:
  `mail-templates/loops/`.

### Service-account protection (`sql/protect-service-account.sql`)
- A Postgres trigger blocks DELETE/role-downgrade of `admin@rokomari.io`'s membership in the
  team org (it powers ingest + invite). Self-healing insert. Applied on prod. **Do not remove
  `admin@rokomari.io` from the org** — it'll just error now, but it's the service identity.

### Client launcher (`rok-vibe-kanban-launcher/`) — NOT yet published to npm
- `@rokomari/vibe-kanban`: sets `VK_SHARED_API_BASE=https://vk.rokomari.io`, pins
  `vibe-kanban-team@0.1.44-20260617110518`, fixed local port **8154** (`BACKEND_PORT`),
  **Node ≥20** guard.
- `install.sh` (NodeSource Node 22 + global install + **systemd `--user`** service, boot+linger),
  `uninstall.sh`, `test-ingest.sh`.

---

## 4. Downstream patches added (in `patches/`)

Submodule `vibe-kanban/` pinned at tag `v0.1.44-20260424091429`. Patch stack applied at build
time via `scripts/apply-patches.sh`. Our additions:
- **0040** — add Zoho sign-in button to `remote-web` `LoginPage.tsx` + `InvitationPage.tsx`
  + `OAuthProvider` type. (Upstream patch 0011 only added Zoho to the *local-web* OAuthDialog;
  `remote-web` was missed — that was the "no Zoho button on the web portal" bug.)
- **0041** — `InvitationCompletePage.tsx` "Get started" now links to `/` (was hardcoded to
  `www.vibekanban.com/docs/getting-started`).

---

## 5. Critical gotchas / lessons (will bite you)

- **OAuth callback path is `/v1/oauth/{provider}/callback`** (provider in the middle), NOT
  `/v1/oauth/callback/{provider}`. Zoho redirect URI = `https://vk.rokomari.io/v1/oauth/zoho/callback`.
- **API enums:** issue `priority` is **lowercase** (`high`); member `role` is **UPPERCASE**
  (`MEMBER`/`ADMIN`).
- **Docker `.env` is NOT shell-sourceable** — values contain spaces (`INGEST_STATUS_NAME=to do`).
  Parse individual keys (`grep '^KEY=' .env | cut -d= -f2-`), don't `source` it.
- **`docker compose exec -T` consumes stdin** — inside `ssh '... ' <<'EOSSH'` heredocs it eats
  the rest of the script. Always add `</dev/null` to such exec calls.
- **Caddy:** changing which file is mounted needs `docker compose up -d --force-recreate caddy`
  (restart doesn't re-bind mounts). Editing mounted-file *content* + `caddy reload` is zero-downtime.
- **Never `rm -rf` a live bind-mounted dir then recreate without `--force-recreate`** — the
  container keeps a stale (empty) mount → 404s. (Hit this once; fixed with force-recreate.)
- **Node ≥20 required** for the npx client (Node 18 → `CustomEvent is not defined`).
- **Versioning quirks:** npm `vibe-kanban-team` versions are timestamped (`0.1.44-<ts>`); there
  is **no bare `0.1.44`** on npm. The Docker **`latest`** tag = `0.1.27` (diverged from npm
  `latest`). The `x-server-version` header is the *remote crate* version (`0.1.27`, constant) —
  NOT the release. Use the asset-bundle hash to tell builds apart.
- **Two frontends:** `remote` serves `packages/remote-web` (built into `/srv/static`); the npx
  client is `packages/local-web` (uses the shared `OAuthDialog`).
- `docker compose` profile services (`ingest`) need `--profile <name>` on **every**
  command (`up`/`pull`), or they get ignored/stopped.

---

## 6. Open items / next steps

1. **Make the frontend fix durable:** build a remote image WITH patches 0040+0041 and pin it
   in prod `.env` (`IMAGE_TAG`), then delete `docker-compose.override.yml` + `remote-web-dist`
   on the VM. Build is wired: `./build.sh` (applies patches → `docker compose
   build remote`) — see §7. Currently the patched frontend runs only via the override stopgap.
2. **Commit / new private repo:** the repo is uncommitted. User intends to **recreate a private
   repo**. Helm/k8s + CI were removed (see §8). To start clean: `rm -rf .git && git init`
   (the existing `.git` is the upstream `iamriajul` history). Artifacts to keep are
   listed in §8.
3. **Loops** email for invitations (optional; manual links work today).
4. **Publish** `@rokomari/vibe-kanban` to an npm registry (public `@rokomari` or internal).
5. **Rotate the Zoho client secret** (it was pasted in a chat during setup).
6. **End-to-end invite test:** `joy@rokomari.com` was invited (pending) — confirm a real Zoho
   accept lands them in the team org.

---

## 7. Build-from-source (docker + vibe-kanban together)

The repo is self-contained: build the images from patched source, then `up`.
```bash
cp .env.example .env            # defaults to local images: vibe-kanban-team-remote:local
# set secrets / PUBLIC_DOMAIN / OAuth in .env
./build.sh                      # submodule init -> apply patches -> docker compose build remote
docker compose up -d
```
- `docker-compose.yml` has `build:` sections (`context: ${VK_DIR:-./vibe-kanban}`,
  `crates/remote/Dockerfile`). Building from source bakes the patched **frontend** in → no override needed.
- Rust release build is slow (~20–40 min). It **has** been run successfully to completion
  on this flattened layout (`./build.sh` success; production-style `docker compose build remote`
  also validated).
- To use a prebuilt image instead, set `REMOTE_IMAGE`/`IMAGE_TAG` in `.env` and skip `build.sh`.

---

## 8. Repo layout (after cleanup)

```
<project root>/                      # single combined root
├── docker-compose.yml               # postgres+remote+electric+caddy (+ingest profile, +build: sections)
├── Caddyfile / Caddyfile.local      # prod (TLS) vs local (http) — select via CADDYFILE_PATH in .env
├── .env.example                     # all config knobs (build-local defaults)
├── build.sh                         # patch + build images
├── init-db/
├── ingest/                          # ingestion sidecar (server.js, Dockerfile, README.md, API.md, test-ingest.sh)
├── memory/                          # org-memory sidecar (server.js, Dockerfile, migrations, README.md)
├── sql/protect-service-account.sql
├── scripts/{apply-patches,update-vibe-kanban,publish-npm,invite,backup}.sh
├── vibe-kanban/                     # upstream source (submodule @ v0.1.44-20260424091429)
├── patches/                         # downstream stack incl. 0040, 0041 + series
├── mail-templates/loops/            # invite/review email templates
├── rok-vibe-kanban-launcher/        # @rokomari/vibe-kanban npx wrapper (install.sh, uninstall.sh)
├── README.md
├── DEPLOYMENT_README.md  GO_LIVE.md
├── REQUIREMENTS_AND_PLAN.md  SYSTEM_DESIGN.md  EXECUTION_PLAN.md  DEVELOPER_ONBOARDING.md
└── HANDOFF.md                       # this file
```

**Removed during cleanup** (Compose-only, going to a new private repo): `helm/`, `.github/`
(all CI), `prompts/`, and Helm-distribution docs (`README/ARCHITECTURE/RELEASE/CLAUDE/AGENTS`
in the old upstream tree).

---

## 9. Common ops (run on the VM unless noted)

```bash
cd ~/vibe-kanban
docker compose --profile ingest ps                       # status (always include profiles in use)
docker compose logs -f remote                             # logs
./scripts/backup.sh                                       # pg_dump -> backups/   (also ~/vk-backup-*.sql.gz exist)
docker compose --profile memory up -d                     # start memory-db + embedder + memory sidecar
./scripts/memory.sh health                                # quick memory health check (/health + /memory/search)
./scripts/invite.sh teammate@rokomari.com member          # invite -> prints accept link
docker compose exec -T postgres psql -U remote -d remote < sql/protect-service-account.sql   # (re)apply guard
# upgrade remote image (keep profile!):  edit IMAGE_TAG in .env then:
docker compose --profile ingest pull remote && docker compose --profile ingest up -d remote
```
Ingest API key:  `ssh -i ~/.ssh/dev ubuntu@103.228.38.106 'grep INGEST_API_KEY /home/ubuntu/vibe-kanban/.env'`

Memory API key: `ssh -i ~/.ssh/dev ubuntu@103.228.38.106 'grep MEMORY_API_KEY /home/ubuntu/vibe-kanban/.env'`
