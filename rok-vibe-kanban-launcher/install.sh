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
BINARIES_BASE_URL="${ROK_VK_BINARIES_BASE_URL:-https://vibe-kanban-binaries.riajul.dev}"
SKIP_BINARY_PREFLIGHT="${ROK_VK_SKIP_BINARY_PREFLIGHT:-0}"
CACHE_BUNDLE_PATH="${ROK_VK_CACHE_BUNDLE_PATH:-}"
CACHE_BUNDLE_URL="${ROK_VK_CACHE_BUNDLE_URL:-}"
CACHE_BUNDLE_BASE_URL="${ROK_VK_CACHE_BUNDLE_BASE_URL:-https://gitlab.rokomari.club/devops/rok-vibe-kanban/-/raw/main/rok-vibe-kanban-launcher/bundles}"
AUTO_CACHE_BUNDLE="${ROK_VK_AUTO_CACHE_BUNDLE:-1}"
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
have systemctl || die "systemd/systemctl is required for user service installation."
have curl || die "curl is required."
have tar || die "tar is required."

detect_platform_dir() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"

  case "$os" in
    Linux) os="linux" ;;
    Darwin) os="macos" ;;
    MINGW*|MSYS*|CYGWIN*) os="windows" ;;
    *) die "Unsupported OS for preloaded cache: ${os}" ;;
  esac

  case "$arch" in
    x86_64|amd64) arch="x64" ;;
    aarch64|arm64) arch="arm64" ;;
    *)
      die "Unsupported architecture for preloaded cache: ${arch}"
      ;;
  esac

  printf '%s-%s' "$os" "$arch"
}

# ---- 1) Node >= 20 ---------------------------------------------------------
node_major() { have node && node -p 'process.versions.node.split(".")[0]' 2>/dev/null || echo 0; }

if [ "$(node_major)" -lt "$NODE_MIN_MAJOR" ]; then
  log "Node >= ${NODE_MIN_MAJOR} not found (have: $(have node && node -v || echo none)). Installing NodeSource Node ${NODE_INSTALL_MAJOR}..."
  have curl || die "curl is required to install Node."
  curl -fsSL "https://deb.nodesource.com/setup_${NODE_INSTALL_MAJOR}.x" | sudo -E bash -
  sudo apt-get install -y nodejs
fi
NODE_BIN="$(command -v node)"
log "Using Node $(node -v) at ${NODE_BIN}"
[ "$(node_major)" -ge "$NODE_MIN_MAJOR" ] || die "Node is still < ${NODE_MIN_MAJOR} after install."

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
sudo npm install -g "$ROK_VK_SOURCE"

# Resolve the absolute bin path so the unit doesn't depend on PATH for ExecStart.
WRAPPER_LINK="$(command -v rok-vibe-kanban)" || die "rok-vibe-kanban not on PATH after install."
WRAPPER_BIN="$(readlink -f "$WRAPPER_LINK")"
log "Wrapper installed at ${WRAPPER_BIN}"

# Resolve pinned client version for both preflight and cache preloading.
WRAPPER_PKG_DIR="$(dirname "$(dirname "$WRAPPER_BIN")")"
VK_TEAM_VERSION="$(
  node -e "const p=require(require.resolve('vibe-kanban-team/package.json',{paths:['$WRAPPER_PKG_DIR']}));process.stdout.write(p.version);"
)" || die "Could not resolve vibe-kanban-team dependency version."
VK_BINARY_TAG="v${VK_TEAM_VERSION}"
VK_PLATFORM_DIR="$(detect_platform_dir)"
EXPECTED_ZIP="${HOME}/.vibe-kanban/bin/${VK_BINARY_TAG}/${VK_PLATFORM_DIR}/vibe-kanban.zip"
CACHE_BUNDLE_FILENAME="rok-vk-cache-${VK_BINARY_TAG}-${VK_PLATFORM_DIR}.tar.gz"

if [ -z "$CACHE_BUNDLE_PATH" ] && [ -z "$CACHE_BUNDLE_URL" ] && [ "$AUTO_CACHE_BUNDLE" = "1" ] && [ -n "$CACHE_BUNDLE_BASE_URL" ]; then
  CACHE_BUNDLE_URL="${CACHE_BUNDLE_BASE_URL%/}/${CACHE_BUNDLE_FILENAME}"
  AUTO_SELECTED_CACHE_BUNDLE=1
  log "Auto-selected cache bundle URL: ${CACHE_BUNDLE_URL}"
else
  AUTO_SELECTED_CACHE_BUNDLE=0
fi

# ---- 2.5) Optional cache preload bundle -------------------------------------
if [ -n "$CACHE_BUNDLE_PATH" ] && [ -n "$CACHE_BUNDLE_URL" ]; then
  die "Set only one of ROK_VK_CACHE_BUNDLE_PATH or ROK_VK_CACHE_BUNDLE_URL."
fi

if [ -n "$CACHE_BUNDLE_PATH" ] || [ -n "$CACHE_BUNDLE_URL" ]; then
  if [ -n "$CACHE_BUNDLE_URL" ]; then
    TMP_BUNDLE="$(mktemp "/tmp/rok-vk-cache-XXXXXX.tar.gz")"
    log "Downloading cache bundle from ${CACHE_BUNDLE_URL}"
    if ! curl -fL "$CACHE_BUNDLE_URL" -o "$TMP_BUNDLE"; then
      if [ "$AUTO_SELECTED_CACHE_BUNDLE" = "1" ]; then
        warn "Auto cache bundle download failed; continuing with normal online flow."
        rm -f "$TMP_BUNDLE" || true
        TMP_BUNDLE=""
        BUNDLE_TO_EXTRACT=""
      else
        die "Failed to download cache bundle."
      fi
    else
      BUNDLE_TO_EXTRACT="$TMP_BUNDLE"
    fi
  else
    [ -f "$CACHE_BUNDLE_PATH" ] || die "Cache bundle not found: ${CACHE_BUNDLE_PATH}"
    BUNDLE_TO_EXTRACT="$CACHE_BUNDLE_PATH"
  fi

  if [ -n "${BUNDLE_TO_EXTRACT:-}" ]; then
    log "Extracting cache bundle to ${HOME}"
    tar -xzf "$BUNDLE_TO_EXTRACT" -C "$HOME" || die "Failed to extract cache bundle."
    if [ -n "${TMP_BUNDLE:-}" ]; then
      rm -f "$TMP_BUNDLE" || true
    fi
  fi
fi

# ---- 2.6) Preflight: verify binaries are reachable or preloaded -------------
if [ -f "$EXPECTED_ZIP" ]; then
  log "Binary cache already available for ${VK_PLATFORM_DIR}: ${EXPECTED_ZIP}"
elif [ "$SKIP_BINARY_PREFLIGHT" != "1" ]; then
  MANIFEST_URL="${BINARIES_BASE_URL}/binaries/v${VK_TEAM_VERSION}/manifest.json"
  log "Preflight check: ${MANIFEST_URL}"

  HTTP_CODE="$(curl -sS -o /dev/null -w '%{http_code}' "$MANIFEST_URL" || true)"
  case "$HTTP_CODE" in
    200|301|302)
      log "Binary manifest is reachable (HTTP ${HTTP_CODE})."
      ;;
    401|403)
      die "Binary manifest is not accessible (HTTP ${HTTP_CODE}) at ${MANIFEST_URL}.
Set ROK_VK_BINARIES_BASE_URL to the correct public/mirrored host, or ask platform team to grant access.
If you intentionally want to continue anyway, re-run with ROK_VK_SKIP_BINARY_PREFLIGHT=1."
      ;;
    *)
      die "Could not reach binary manifest (HTTP ${HTTP_CODE}) at ${MANIFEST_URL}.
Check network/DNS/firewall/proxy on this device, or re-run with ROK_VK_SKIP_BINARY_PREFLIGHT=1."
      ;;
  esac
else
  warn "Skipping binary preflight because ROK_VK_SKIP_BINARY_PREFLIGHT=1"
fi

# ---- 3) systemd --user unit ------------------------------------------------
UNIT_DIR="${HOME}/.config/systemd/user"
UNIT_FILE="${UNIT_DIR}/${SERVICE_NAME}.service"
mkdir -p "$UNIT_DIR"

log "Writing ${UNIT_FILE}"
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
# systemd --user starts with a minimal PATH; add node, global npm bin, and the
# user-local bin where AI CLIs (claude, etc.) install.
Environment=PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:%h/.local/bin
ExecStart=${NODE_BIN} ${WRAPPER_BIN}
Restart=on-failure
# The launcher exits with 42 for known non-retryable auth failures
# (binary manifest endpoint returns HTTP 401).
RestartPreventExitStatus=42
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
