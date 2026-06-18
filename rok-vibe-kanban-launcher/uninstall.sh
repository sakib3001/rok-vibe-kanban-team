#!/usr/bin/env bash
#
# Rokomari Vibe Kanban — uninstaller. Reverses install.sh.
#
# Default: stop + disable the user service, remove the unit, remove the global
# wrapper package. Leaves Node, linger, and your local data/credentials alone.
#
# Flags:
#   --purge            also delete local client data + credentials
#                      (~/.local/share/vibe-kanban, ~/.vibe-kanban,
#                       ~/.config/vibe-kanban, ~/.cache/vibe-kanban)
#   --disable-linger   turn off user lingering (only do this if no other
#                      --user services rely on it)
#   --keep-package     don't remove the global @rokomari/vibe-kanban package
#
set -euo pipefail

SERVICE_NAME="${ROK_VK_SERVICE_NAME:-vibe-kanban}"
PKG="@rokomari/vibe-kanban"
PURGE=0; DISABLE_LINGER=0; KEEP_PKG=0

for arg in "$@"; do
  case "$arg" in
    --purge) PURGE=1 ;;
    --disable-linger) DISABLE_LINGER=1 ;;
    --keep-package) KEEP_PKG=1 ;;
    -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
    *) printf 'Unknown flag: %s\n' "$arg" >&2; exit 2 ;;
  esac
done

log()  { printf '\033[1;34m[uninstall]\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[uninstall]\033[0m %s\n' "$*" >&2; }
have() { command -v "$1" >/dev/null 2>&1; }

[ "$(id -u)" -ne 0 ] || { echo "Run as your normal user, not root." >&2; exit 1; }

UNIT_FILE="${HOME}/.config/systemd/user/${SERVICE_NAME}.service"

# ---- Stop + disable + remove the unit --------------------------------------
if systemctl --user list-unit-files "${SERVICE_NAME}.service" >/dev/null 2>&1; then
  log "Stopping and disabling ${SERVICE_NAME}.service..."
  systemctl --user disable --now "${SERVICE_NAME}.service" 2>/dev/null || true
fi
if [ -f "$UNIT_FILE" ]; then
  log "Removing ${UNIT_FILE}"
  rm -f "$UNIT_FILE"
fi
systemctl --user daemon-reload 2>/dev/null || true
systemctl --user reset-failed "${SERVICE_NAME}.service" 2>/dev/null || true

# ---- Remove the global wrapper package -------------------------------------
if [ "$KEEP_PKG" -eq 0 ]; then
  if have npm && npm ls -g --depth 0 "$PKG" >/dev/null 2>&1; then
    log "Removing global package ${PKG}..."
    sudo npm rm -g "$PKG" || warn "Could not remove ${PKG} (continue anyway)."
  else
    log "Global package ${PKG} not installed; skipping."
  fi
else
  log "Keeping global package (--keep-package)."
fi

# ---- Optional: disable linger ----------------------------------------------
if [ "$DISABLE_LINGER" -eq 1 ]; then
  log "Disabling user lingering..."
  sudo loginctl disable-linger "$USER" || warn "Could not disable linger."
else
  log "Leaving user lingering enabled (use --disable-linger to turn it off)."
fi

# ---- Optional: purge local data --------------------------------------------
if [ "$PURGE" -eq 1 ]; then
  warn "Purging local client data and credentials..."
  for d in \
    "$HOME/.local/share/vibe-kanban" \
    "$HOME/.vibe-kanban" \
    "$HOME/.config/vibe-kanban" \
    "$HOME/.cache/vibe-kanban"; do
    [ -e "$d" ] && { log "  rm -rf $d"; rm -rf "$d"; }
  done
  warn "You will need to sign in again next time you install."
else
  log "Keeping local data/credentials (use --purge to delete)."
fi

echo
log "✅ Uninstalled. (Node was left installed; remove it manually if you want.)"
