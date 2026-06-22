# Go-Live Guide — vk.rokomari.io

> OAuth and Harbor are **deferred**. Production runs on GHCR images with **bootstrap local auth**.

## Current status

| Item | Status |
|------|--------|
| Domain + TLS | `https://vk.rokomari.io` |
| Health | `GET /v1/health` → `{"status":"ok",...}` |
| Stack | postgres + remote + electric + caddy (healthy) |
| Docker on boot | enabled |
| Auth (temporary) | bootstrap local admin (see below) |
| OAuth (GitHub/Google) | deferred |
| Harbor registry | deferred |
| Image pin | pin digest/tag in `.env` before next upgrade |
| Team org / project | "Rokomari SE Team" / **Amaly** |
| Ingestion API | **live** — `POST /ingest/issues` (`--profile ingest`); see [ingest/README.md](./ingest/README.md) |
| Memory layer | optional — `--profile memory` (`/memory/*` MCP/REST + pgvector + ollama) |
| Client launcher | `@rokomari/vibe-kanban` (in `../rok-vibe-kanban-launcher/`), Node ≥ 20, port 8154 — publish pending |

---

## 1) First login (bootstrap admin)

Open: https://vk.rokomari.io/account

Use the credentials configured in server `.env`:

- Email: value of `SELF_HOST_LOCAL_AUTH_EMAIL`
- Password: value of `SELF_HOST_LOCAL_AUTH_PASSWORD`

After login you should reach the main app (not stuck on `/account`).

---

## 2) Create team org + project (Phase 2)

In the UI:

1. Create a **team organisation** (not personal-only workflow long term).
2. Create the first **project** and kanban columns (To Do, In Progress, In Review, Done).
3. Note the org slug — developers will join this org.

Invites: email is disabled unless `LOOPS_EMAIL_API_KEY` is set. For now, share invite links manually from the org invitations UI.

---

## 3) Connect local developer client

**Requires Node ≥ 20** (Node 18 fails with `CustomEvent is not defined`).

Preferred — the standardized launcher (clone `rok-vibe-kanban-launcher/`, then):

```bash
./install.sh        # Node 22 + pinned client + systemd --user service on port 8154
# or run on demand:  npx @rokomari/vibe-kanban   (once published)
```

Pre-publish fallback — point the stock client at the central server:

```bash
VK_SHARED_API_BASE=https://vk.rokomari.io npx vibe-kanban
```

See [DEVELOPER_ONBOARDING.md](./DEVELOPER_ONBOARDING.md). Sign in with the bootstrap
credentials (or OAuth when enabled). Agent execution stays local on the developer machine.

---

## 4) Operations on the server

```bash
cd ~/vibe-kanban

# status
docker compose ps
curl -fsS https://vk.rokomari.io/v1/health

# logs
docker compose logs -f remote

# backup (writes backups/remote-YYYY-MM-DD.sql.gz)
./scripts/backup.sh

# upgrade (after editing IMAGE_TAG in .env)
# NOTE: include all active profiles so sidecars stay managed/running.
docker compose --profile ingest --profile memory pull && docker compose --profile ingest --profile memory up -d
```

> The ingestion API runs under the `ingest` profile. Any `docker compose` command that
> should keep it running must include `--profile ingest` (a plain `up -d`/`down` ignores
> or stops it). API key + setup: [ingest/README.md](./ingest/README.md).
>
> The memory layer follows the same profile rule (`--profile memory`) and is routed at
> `/memory/*`. Keep both profile flags whenever both sidecars are enabled.

---

## 5) Deferred (do later)

- **OAuth (GitHub/Google)**: set `GITHUB_OAUTH_CLIENT_ID` / `GITHUB_OAUTH_CLIENT_SECRET` (or the `GOOGLE_*` pair), callback `https://vk.rokomari.io/v1/oauth/github/callback`, then clear bootstrap auth vars.
- **Harbor images**: set `REMOTE_IMAGE` and pinned `IMAGE_TAG` in `.env`.
- **Phase 1.5**: publish the `@rokomari/vibe-kanban` wrapper (already built in `../rok-vibe-kanban-launcher/`) to the npm registry.
- **Phase 4**: dispatch CLI for leads.
- **Memory profile**: enable org memory retrieval and ingestion with `docker compose --profile memory up -d`,
  then configure `MEMORY_*` and `EMBED_*` vars in `.env`.

---

## 6) Acceptance checklist

- [ ] Bootstrap login works in browser
- [ ] Team org + project created
- [ ] Local `npx` client connects with `VK_SHARED_API_BASE`
- [ ] Issue created and persists after `docker compose restart`
- [ ] `./scripts/backup.sh` produces a `.sql.gz` file
