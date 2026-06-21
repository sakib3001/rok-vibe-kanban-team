#!/usr/bin/env bash
#
# Rokomari Vibe Kanban — one-shot developer installer.
#
# Idempotent. Safe to re-run (e.g. to upgrade). It will:
#   1. Ensure Node >= 20 (installs NodeSource Node 22 if needed; needs sudo).
#   2. Globally install the pinned @rokomari/vibe-kanban wrapper.
#   3. Install a systemd --user service that runs the client on a fixed port.
#   4. Enable it on boot (linger) and start it now.
#
# Run from the launcher repo (installs the local wrapper, good for pre-publish):
#   ./install.sh
# Or, once the wrapper is published, from anywhere:
#   ROK_VK_SOURCE='@rokomari/vibe-kanban@1.0.0' curl -fsSL .../install.sh | bash
#
set -euo pipefail

# ---- Config (override via env) ---------------------------------------------
SERVICE_NAME="${ROK_VK_SERVICE_NAME:-vibe-kanban}"
VK_PORT="${ROK_VK_PORT:-8154}"
CENTRAL_API_BASE="${VK_SHARED_API_BASE:-https://vk.rokomari.io}"
VK_BINARIES_BASE_URL="${ROK_VK_BINARIES_BASE_URL:-https://rokfiles.rokomari.io}"
NODE_MIN_MAJOR=20
NODE_INSTALL_MAJOR=22
# Where to install the wrapper from. Default: this repo if it looks like the
# launcher, otherwise the published package.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROK_VK_SOURCE="${ROK_VK_SOURCE:-}"
# ----------------------------------------------------------------------------

log()  { printf '\033[1;34m[install]\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[install]\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31m[install] ERROR:\033[0m %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

[ "$(id -u)" -ne 0 ] || die "Run as your normal user, not root (the service runs as you)."
have sudo || die "sudo is required (for Node install + global npm install)."

# ---- 1) Node >= 20 ---------------------------------------------------------
node_major() { have node && node -p 'process.versions.node.split(".")[0]' 2>/dev/null || echo 0; }

if [ "$(node_major)" -lt "$NODE_MIN_MAJOR" ]; then
  log "Node >= ${NODE_MIN_MAJOR} not found (have: $(have node && node -v || echo none)). Installing NodeSource Node ${NODE_INSTALL_MAJOR}..."
  have curl || die "curl is required to install Node."
  curl -fsSL "https://deb.nodesource.com/setup_${NODE_INSTALL_MAJOR}.x" | sudo -E bash -
  sudo apt-get install -y nodejs
fi
NODE_BIN="$(command -v node)"
NPM_BIN="$(command -v npm || true)"
NODE_DIR="$(dirname "$NODE_BIN")"
NPM_DIR="$(dirname "${NPM_BIN:-$NODE_BIN}")"
log "Using Node $(node -v) at ${NODE_BIN}"
[ "$(node_major)" -ge "$NODE_MIN_MAJOR" ] || die "Node is still < ${NODE_MIN_MAJOR} after install."
[ -n "$NPM_BIN" ] || die "npm not found on PATH for user ${USER}. If using nvm, run 'nvm use <version>' first."
log "Using npm $(npm -v) at ${NPM_BIN}"

# ---- 2) Install the wrapper globally ---------------------------------------
if [ -z "$ROK_VK_SOURCE" ]; then
  if [ -f "${SCRIPT_DIR}/package.json" ] && grep -q '"@rokomari/vibe-kanban"' "${SCRIPT_DIR}/package.json"; then
    ROK_VK_SOURCE="${SCRIPT_DIR}"
    log "Installing wrapper from local repo: ${ROK_VK_SOURCE}"
  else
    ROK_VK_SOURCE="@rokomari/vibe-kanban"
    log "Installing wrapper from registry: ${ROK_VK_SOURCE}"
  fi
fi
sudo env "PATH=${NODE_DIR}:$PATH" "$NPM_BIN" install -g "$ROK_VK_SOURCE"

# Resolve the absolute bin path so the unit doesn't depend on PATH for ExecStart.
WRAPPER_LINK="$(command -v rok-vibe-kanban)" || die "rok-vibe-kanban not on PATH after install."
WRAPPER_BIN="$(readlink -f "$WRAPPER_LINK")"
log "Wrapper installed at ${WRAPPER_BIN}"

# Patch the bundled vibe-kanban-team binary host to our R2 domain.
WRAPPER_PKG_DIR="$(dirname "$(dirname "$WRAPPER_BIN")")"
TEAM_PKG_DIR="${WRAPPER_PKG_DIR}/node_modules/vibe-kanban-team"
if [ -d "$TEAM_PKG_DIR" ]; then
  log "Patching binary host in ${TEAM_PKG_DIR}"
  sudo "$NODE_BIN" -e '
const fs = require("fs");
const path = require("path");
const root = process.argv[1];
const newUrl = process.argv[2];
const oldHostRe = /https:\/\/vibe-kanban-binaries\.[^"'\''\s]+/g;
let patchedFiles = 0;
let visitedJs = 0;

function walk(dir) {
  for (const name of fs.readdirSync(dir)) {
    const full = path.join(dir, name);
    const st = fs.statSync(full);
    if (st.isDirectory()) {
      walk(full);
      continue;
    }
    if (!name.endsWith(".js")) continue;
    visitedJs += 1;
    const src = fs.readFileSync(full, "utf8");
    const next = src.replace(oldHostRe, newUrl);
    if (next !== src) {
      fs.writeFileSync(full, next);
      patchedFiles += 1;
    }
  }
}

walk(root);
if (patchedFiles === 0) {
  console.error(`[install] WARN: no JS file required binary host patch under ${root} (scanned ${visitedJs} JS files)`);
} else {
  console.log(`[install] patched ${patchedFiles} JS files under ${root}`);
}
' "$TEAM_PKG_DIR" "$VK_BINARIES_BASE_URL"
else
  warn "Could not find vibe-kanban-team package to patch: ${TEAM_PKG_DIR}"
fi

# ---- 3) systemd --user unit ------------------------------------------------
UNIT_DIR="${HOME}/.config/systemd/user"
UNIT_FILE="${UNIT_DIR}/${SERVICE_NAME}.service"
mkdir -p "$UNIT_DIR"

log "Writing ${UNIT_FILE}"
# Include discovered node/npm paths so the wrapper can spawn npm/npx under
# systemd's restricted environment.
SYSTEMD_PATH="${NODE_DIR}:${NPM_DIR}:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:%h/.local/bin"
cat > "$UNIT_FILE" <<EOF
[Unit]
Description=Vibe Kanban local client (Rokomari)
Documentation=https://vk.rokomari.io
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
# Central server + fixed local UI port (see launcher README).
Environment=VK_SHARED_API_BASE=${CENTRAL_API_BASE}
Environment=BACKEND_PORT=${VK_PORT}
Environment=ROK_VK_TEAM_ROOT=${TEAM_PKG_DIR}
# Keep fallback disabled in production; it can pull an unpatched package.
Environment=ROK_VK_ALLOW_NPX_FALLBACK=0
# systemd --user starts with a minimal PATH; add node, global npm bin, and the
# user-local bin where AI CLIs (claude, etc.) install.
Environment=PATH=${SYSTEMD_PATH}
ExecStart=${NODE_BIN} ${WRAPPER_BIN}
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
EOF

# ---- 4) Enable on boot (linger) + start now --------------------------------
log "Enabling lingering so the service runs at boot without an active login..."
sudo loginctl enable-linger "$USER"

systemctl --user daemon-reload
systemctl --user enable --now "${SERVICE_NAME}.service"

# ---- Done ------------------------------------------------------------------
sleep 2
echo
log "Service status:"
systemctl --user --no-pager --full status "${SERVICE_NAME}.service" | sed -n '1,8p' || true
echo
log "✅ Installed. Local UI:  http://127.0.0.1:${VK_PORT}"
echo
cat <<NOTE
Next steps / useful commands:
  - Open the UI:        http://127.0.0.1:${VK_PORT}
  - Logs:               journalctl --user -u ${SERVICE_NAME} -f
  - Restart / stop:     systemctl --user restart ${SERVICE_NAME}
                        systemctl --user stop ${SERVICE_NAME}
  - Upgrade later:      re-run this script (re-installs wrapper + unit)

One-time (interactive) step the service cannot do for you:
  Authenticate your AI CLI(s) once, e.g.:  claude    (then sign in)
  After that the background service can run agents using the stored credentials.
NOTE
