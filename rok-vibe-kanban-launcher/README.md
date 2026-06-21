# @rokomari/vibe-kanban — standardized launcher

The **one command** every Rokomari developer runs to use Vibe Kanban. It pins the
client version and points it at the central server (`https://vk.rokomari.io`) with
**zero manual configuration** — no env vars, no base-URL setup.

```bash
npx @rokomari/vibe-kanban
```

That's it. The launcher sets `VK_SHARED_API_BASE=https://vk.rokomari.io` and runs the
pinned `vibe-kanban-team` client. AI agents, worktrees, and terminals run on the
developer's own machine; all shared state lives on the central server.

## Why a wrapper (not raw `npx vibe-kanban-team`)

Running the upstream client directly would require every developer to remember
`VK_SHARED_API_BASE=https://vk.rokomari.io npx vibe-kanban-team@<version>` — error-prone
and unpinned. This wrapper bakes both in, so the command is identical and reproducible
across the whole team.

## One-shot install + run as a service (recommended for onboarding)

`install.sh` sets everything up and runs the client as a **user-level systemd service**
(always-on, auto-restart, starts at boot):

```bash
cd rok-vibe-kanban-launcher
./install.sh
```

It will (idempotently):
1. Ensure **Node ≥ 20** (installs NodeSource Node 22 if needed — uses `sudo`).
2. Globally install the pinned wrapper (from this repo pre-publish, or the registry once published).
3. Preflight-check binary manifest reachability for the pinned `vibe-kanban-team` version.
4. Write `~/.config/systemd/user/vibe-kanban.service` with the central URL + fixed port.
5. `loginctl enable-linger` + `systemctl --user enable --now` so it runs at boot.

After install:

| Action | Command |
|--------|---------|
| Open UI | `http://127.0.0.1:8154` |
| Logs | `journalctl --user -u vibe-kanban -f` |
| Restart / stop | `systemctl --user restart vibe-kanban` / `stop vibe-kanban` |
| Upgrade | re-run `./install.sh` |
| Uninstall | `./uninstall.sh` (add `--purge` to also remove local data/credentials) |

If startup fails because the binary host returns `HTTP 401` for `manifest.json`,
the launcher exits with a non-retryable code and systemd will not crash-loop.
Check logs with `journalctl --user -u vibe-kanban -n 50 --no-pager`.

Useful installer overrides for cross-device environments:
- `ROK_VK_BINARIES_BASE_URL=https://...` to point at a reachable mirror/binary host.
- `ROK_VK_SKIP_BINARY_PREFLIGHT=1` to bypass the manifest reachability check.
- `ROK_VK_CACHE_BUNDLE_PATH=/path/to/rok-vk-cache-v<version>-<platform>.tar.gz` to preload client binaries from a local file.
- `ROK_VK_CACHE_BUNDLE_URL=https://.../rok-vk-cache-v<version>-<platform>.tar.gz` to preload client binaries from a URL.
- `ROK_VK_CACHE_BUNDLE_BASE_URL=https://.../bundles` to auto-resolve a version/platform bundle URL.
- `ROK_VK_AUTO_CACHE_BUNDLE=0` to disable automatic cache-bundle URL probing.

### Offline/cross-device cache bundle (recommended for restricted networks)

On a machine where `rok-vibe-kanban` already runs successfully:

```bash
cd rok-vibe-kanban-launcher
./make-cache-bundle.sh
```

This creates `bundles/rok-vk-cache-v<version>-<platform>.tar.gz` with the pinned
client cache under `.vibe-kanban/`.

### GitLab-hosted automatic bundle (no extra flags on developer PCs)

Commit the generated tarball(s) to:

`rok-vibe-kanban-launcher/bundles/`

`install.sh` auto-tries:

`https://gitlab.rokomari.club/devops/rok-vibe-kanban/-/raw/main/rok-vibe-kanban-launcher/bundles/rok-vk-cache-v<version>-<platform>.tar.gz`

If found, it preloads binaries automatically. If not found, installer falls back
to the normal online flow.

Copy/upload that tarball to the target machine, then install with:

```bash
ROK_VK_CACHE_BUNDLE_PATH=/path/to/rok-vk-cache-v<version>-<platform>.tar.gz ./install.sh
```

If the target machine can only fetch from HTTP:

```bash
ROK_VK_CACHE_BUNDLE_URL=https://<host>/rok-vk-cache-v<version>-<platform>.tar.gz ./install.sh
```

**One-time manual step the service can't do:** authenticate your AI CLI(s) interactively
once (e.g. run `claude` and sign in). The background service then uses the stored creds.

> Why a `--user` service (not system-wide): the client runs agents **as you** — it needs
> your home, git identity, SSH keys, repos, and AI-CLI auth. A root/system unit would have
> none of those.

## How it works

- `bin/rok-vibe-kanban.js` injects `VK_SHARED_API_BASE` (unless already set) and
  delegates to the pinned `vibe-kanban-team` dependency.
- The client version is pinned in `package.json` (`dependencies.vibe-kanban-team`), so
  `npx @rokomari/vibe-kanban` always resolves to one known-good build.
- Any extra args are forwarded to the underlying client.

### Fixed local port

The local UI runs on a **fixed port `8154`** (`http://127.0.0.1:8154`) so the URL is
stable across runs. Without this the client picks a random free port each launch.
Override if 8154 is taken:

```bash
ROK_VK_PORT=9000 npx @rokomari/vibe-kanban
```

### Pointing at a non-prod server (testing only)

```bash
VK_SHARED_API_BASE=https://staging-vk.rokomari.io npx @rokomari/vibe-kanban
```

## Version pinning & upgrade policy

**Rule:** the pinned `vibe-kanban-team` version must match the **upstream build of the
deployed remote image** (`docker/.env` → `IMAGE_TAG`). Client and server move together.

> **npm version scheme:** releases are published as `<upstream-semver>-<YYYYMMDDHHmmss>`
> (e.g. `0.1.44-20260617110518`). There is **no** bare `0.1.44` on npm — always pin the
> full timestamped string. Find the newest with: `npm view vibe-kanban-team dist-tags`.

Current pin: **`0.1.44-20260617110518`** (npm `latest` for the `0.1.44` upstream base).

To upgrade the team:

1. Bump and pin the remote image tag in the server `docker/.env` (`IMAGE_TAG`), redeploy,
   and verify health.
2. Bump `dependencies.vibe-kanban-team` here to the **same** upstream semver.
3. Bump this wrapper's own `version` (e.g. `1.0.0` → `1.0.1`).
4. Publish (below). Developers automatically get the new client on their next `npx` run.

> Never let the deployed server and the pinned client drift across a major/minor upstream
> version — protocol/schema mismatches can break sync or auth.

## Publishing

Choose the registry that matches Rokomari's npm setup:

**Public npm (scoped, requires the `@rokomari` org):**
```bash
cd rok-vibe-kanban-launcher
npm publish --access public
```

**Internal registry (recommended for an internal tool):**
```bash
# .npmrc (committed at repo root or ~/.npmrc):
#   @rokomari:registry=https://npm.rokomari.io
#   //npm.rokomari.io/:_authToken=${NPM_TOKEN}
cd rok-vibe-kanban-launcher
npm publish
```

If using an internal registry, developers need the matching `@rokomari:registry` line in
their `~/.npmrc` for `npx @rokomari/vibe-kanban` to resolve. Document this in onboarding.

## Clean-machine smoke test (Phase 1.5 acceptance)

On a machine that has **never** run this before (or after `npm cache clean --force`):

```bash
npx @rokomari/vibe-kanban
```

Expected:
1. Launcher prints `launching vibe-kanban-team@0.1.44-20260617110518 -> https://vk.rokomari.io`.
2. Client starts and opens the UI; sign-in uses the central OAuth flow.
3. After sign-in, the developer's assigned issues are visible.
4. Starting an issue runs an agent **locally** with state reflected centrally.

## Requirements

- **Node.js ≥ 20** (20 LTS or newer; the client is built against Node 24). Node 18 fails
  with `CustomEvent is not defined`. The launcher enforces this and exits early with a hint.
- Network access to `https://vk.rokomari.io` and the npm registry hosting this package.
