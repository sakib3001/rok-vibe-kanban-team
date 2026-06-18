# Vibe Kanban — Developer Onboarding

How to get on the team's centralized Vibe Kanban (`https://vk.rokomari.io`) and complete
your first assigned task. Everything runs as a **local client** on your machine, connected
to the shared central server.

> Companion docs: [EXECUTION_PLAN.md](./EXECUTION_PLAN.md),
> [SYSTEM_DESIGN.md](./SYSTEM_DESIGN.md), launcher [README](./rok-vibe-kanban-launcher/README.md).

---

## 1) Prerequisites

| # | Requirement | How to get it / check |
|---|-------------|-----------------------|
| 1 | **Linux/macOS dev machine** | — |
| 2 | **Node.js ≥ 20** (20 LTS+; client is built against Node 24) | The installer handles this on Ubuntu (NodeSource Node 22). Manual check: `node -v`. Node 18 fails with `CustomEvent is not defined`. |
| 3 | **Git installed + identity configured** | `git --version`; `git config --global user.name` / `user.email` set |
| 4 | **Network access to `https://vk.rokomari.io`** | `curl -fsS https://vk.rokomari.io/health` → `200`. On VPN if required. |
| 5 | **A Rokomari account in the org** | You must be in the **"Rokomari SE Team"** org (ask a lead). *Current:* a shared bootstrap admin login (OAuth `@rokomari.com` is being enabled — a lead will give you sign-in details). |
| 6 | **At least one AI coding CLI, authenticated** | e.g. Claude Code: install, then run `claude` once and sign in. Needed to actually run agents (one-time, interactive). |
| 7 | **sudo** (only for first install: Node + global npm) | passwordless or your password |

---

## 2) Install (one command)

```bash
git clone <this-repo> && cd <repo>/rok-vibe-kanban-launcher
./install.sh
```

This installs Node 22 (if needed), the pinned client wrapper, and a **user-level systemd
service** that runs the client at a fixed port on boot. When it finishes:

- Open **http://127.0.0.1:8154**
- Logs: `journalctl --user -u vibe-kanban -f`

> Prefer not to run it as a service? You can launch on demand instead:
> `npx @rokomari/vibe-kanban` (same central URL + port baked in).

---

## 3) First task

1. Open **http://127.0.0.1:8154** and **sign in** with your `@rokomari.com` account.
2. Confirm you're in the **Rokomari SE Team** org (switch org if needed).
3. Find an issue **assigned to you** (your personal queue).
4. Open it and **start a workspace** — the client creates a git worktree on your machine
   and runs the AI agent **locally**.
5. Work the task; status and updates sync back to the central server in real time.

---

## 4) One-time AI-CLI authentication

The background service can't do an interactive login for you. Once, in a terminal:

```bash
claude            # sign in when prompted (or your chosen AI CLI)
```

After that the service runs agents using the stored credentials.

---

## 5) Common issues

| Symptom | Cause | Fix |
|---------|-------|-----|
| `CustomEvent is not defined` | Node < 20 | Install Node ≥ 20 (the installer does this) |
| `npm install` 404 on `0.1.44` | bare version isn't published | use the pinned `0.1.44-<timestamp>` (already pinned in the wrapper) |
| Random local port each run | port not fixed | wrapper pins `BACKEND_PORT=8154`; override with `ROK_VK_PORT` |
| Can't sign in | not invited / wrong email domain | ask a lead to invite your `@rokomari.com` address |
| Service won't start | systemd `--user` PATH/Node | check `journalctl --user -u vibe-kanban`; re-run `./install.sh` |
| Agent won't run | AI CLI not authenticated | run `claude` once and sign in |

---

## 6) Manage / remove

| Action | Command |
|--------|---------|
| Restart / stop | `systemctl --user restart vibe-kanban` / `stop vibe-kanban` |
| Upgrade | re-run `./install.sh` |
| Uninstall | `./uninstall.sh` (add `--purge` to also remove local data + credentials) |
