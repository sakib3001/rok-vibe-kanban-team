# Centralized Vibe Kanban — Docker Compose Deployment

Self-hosted central backend for a team that runs **local `npx` clients**. No
Kubernetes required. Stack: PostgreSQL + ElectricSQL + Remote Server + Caddy (TLS).

> Why no browser UI here? The browser "frontend pod" needs the sysbox container
> runtime and is Kubernetes-only. This Compose stack serves the **API + sync**
> that local clients connect to — which is the model we chose. See
> [`../SYSTEM_DESIGN.md`](../SYSTEM_DESIGN.md) §8.

## Prerequisites

- A Linux host with Docker + Docker Compose v2
- The `vibe-kanban/` source (in this same repo root) — for building from source
- A domain name with an **A record → this host's public IP**
- Ports **80 and 443** open (Caddy needs them for Let's Encrypt + serving)
- One OAuth app (GitHub, Google, or Zoho)

## Setup

```bash
cp .env.example .env

# generate secrets
echo "POSTGRES_PASSWORD=$(openssl rand -base64 24 | tr -d '/+=')"
echo "ELECTRIC_ROLE_PASSWORD=$(openssl rand -base64 24 | tr -d '/+=')"
echo "JWT_SECRET=$(openssl rand -base64 32)"     # must stay valid base64 (>=32 bytes)
# paste these into .env, then set PUBLIC_DOMAIN, ACME_EMAIL, OAuth creds
```

## Build from source, then up (recommended)

The remote image is built from the patched `vibe-kanban/` source — so the image
contains the patched backend **and** the patched `remote-web` frontend (Zoho buttons,
invite-complete redirect). No registry pull, no frontend override needed.

```bash
./build.sh                 # init submodule -> apply patch stack -> docker compose build remote
docker compose up -d
```

`build.sh` tags the image to match the compose `image:` field (`vibe-kanban-team-remote:local`
by default, from `.env`). The `vibe-kanban` source path can be overridden with `VK_DIR=...`.

> Alternative: to run a **prebuilt registry image** instead of building, set `REMOTE_IMAGE` /
> `IMAGE_TAG` in `.env` to that image and skip `build.sh` (just `docker compose up -d`).

## Phase 1 production profile (vk.rokomari.io + Harbor)

```bash
cp .env.production.example .env

# Generate and set strong secrets in .env
# POSTGRES_PASSWORD / ELECTRIC_ROLE_PASSWORD / JWT_SECRET
```

For Harbor image pulls on the remote VM:

```bash
docker login harbor.rokomari.io
docker pull harbor.rokomari.io/vibe-kanban/vibe-kanban-team-remote:<pinned-tag>
```

Use a pinned `IMAGE_TAG` in `.env` before go-live.

### Register the OAuth callback

For your provider, set the callback URL to:

```
https://<PUBLIC_DOMAIN>/v1/oauth/callback/github     # or /google, /zoho
```

## Run

```bash
docker compose up -d
docker compose ps
docker compose logs -f remote     # watch migrations + startup
```

Startup order is handled automatically: `postgres` (healthy) → `remote` (runs
migrations) → `electric-init` waits for `/health` → `electric` starts.

### Health checks

```bash
curl -fsS https://<PUBLIC_DOMAIN>/v1/health     # remote API (through Caddy)
docker compose exec electric wget -qO- http://localhost:3000/v1/health   # sync layer
```

## Connect a developer (local client)

Developers use the **`@rokomari/vibe-kanban` launcher** (see
[`../rok-vibe-kanban-launcher/`](../rok-vibe-kanban-launcher/) and
[`../DEVELOPER_ONBOARDING.md`](../DEVELOPER_ONBOARDING.md)) — it pins the client, bakes in
the central URL, and runs on fixed port 8154. Requires **Node ≥ 20**.

```bash
# once published:
npx @rokomari/vibe-kanban
# pre-publish / no wrapper: point the stock client at the central server
VK_SHARED_API_BASE=https://<PUBLIC_DOMAIN> npx vibe-kanban
```

The developer signs in, joins the org, and sees issues assigned to them.
Agents/worktrees run on their own machine.

## Optional profiles

The base stack is `postgres + remote + electric + caddy`. One opt-in add-on ships as
compose **profiles** (so the base stack is unaffected when they're off):

```bash
docker compose --profile ingest up -d   # POST /ingest/issues -> auto-create issues
```

- **ingest** — issue-ingestion API; see [`ingest/README.md`](./ingest/README.md). Caddy already
  routes `/ingest/*` to it. Requires `INGEST_*` in `.env` + a service account in the team org.

> ⚠️ **Always repeat the profile flag** for compose ops once a profile is in use, e.g.
> `docker compose --profile ingest up -d`. A plain `docker compose up -d` won't manage the
> profiled container, and `docker compose down` stops it without bringing it back.

## Operations

| Action | Command |
|--------|---------|
| Update images | edit `*_TAG` in `.env` → `docker compose pull && docker compose up -d` |
| Backup DB | `docker compose exec postgres pg_dump -U remote remote > backup-$(date +%F).sql` |
| Restore DB | `cat backup.sql \| docker compose exec -T postgres psql -U remote remote` |
| Logs | `docker compose logs -f <service>` |
| Stop | `docker compose down` (data persists in volumes) |
| Wipe (DESTRUCTIVE) | `docker compose down -v` |

**Back up the `pgdata` volume regularly and test a restore** (see EXECUTION_PLAN P5-T3).

## Local testing (no domain / no real certs)

Set in `.env`:

- `CADDYFILE_PATH=Caddyfile.local`
- `SERVER_PUBLIC_BASE_URL=http://localhost`
- either one OAuth provider configured for localhost OR bootstrap local auth vars

Then run `docker compose up -d`.

## Troubleshooting

- **Migration fails with `electric_sync already exists`** — this happens if both
  bootstrap SQL and app migrations create the role. This Compose stack expects role
  creation via app migrations only. If you hit this on an older volume, recreate DB
  volume (`docker compose down -v`) for clean initialization.
- **OAuth redirect mismatch** — `SERVER_PUBLIC_BASE_URL` must exactly equal
  `https://<PUBLIC_DOMAIN>` and match the provider's registered callback.
- **`wal_level` errors** — only happens if you replaced Postgres; this stack sets
  `wal_level=logical` via the `postgres` command already.
- **JWT startup failure** — `JWT_SECRET` must be standard base64 decoding to ≥32
  bytes. Use `openssl rand -base64 32`, not a plain alphanumeric string.
- **Caddy config change not applied** — changing `CADDYFILE_PATH` (or which file is
  mounted) needs `docker compose up -d caddy` (**recreate**), not `restart`. Editing the
  *content* of the already-mounted file is zero-downtime: `docker compose exec caddy caddy
  reload --config /etc/caddy/Caddyfile --adapter caddyfile`.
- **Client `CustomEvent is not defined`** — the developer's Node is < 20. The client needs
  **Node ≥ 20**.
