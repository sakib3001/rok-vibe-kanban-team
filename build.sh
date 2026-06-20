#!/usr/bin/env bash
#
# Build the Vibe Kanban images from the patched upstream source, then run
# `docker compose up -d`. This applies the downstream patch stack first, so the
# built image includes the patched backend AND the patched remote-web frontend
# (Zoho buttons, invite-complete redirect, etc.) — no separate override needed.
#
# Usage:
#   ./build.sh            # build the remote image
#
# Override the source location with VK_DIR=/path/to/vibe-kanban
#
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
cd "$HERE"

[ -f .env ] || { echo "No .env found — run: cp .env.example .env  (then edit it)"; exit 1; }

VK_DIR_RAW="${VK_DIR:-$HERE/vibe-kanban}"
VK_DIR="$(cd "$VK_DIR_RAW" 2>/dev/null && pwd)" || {
  echo "vibe-kanban source not found at: $VK_DIR_RAW (set VK_DIR=/path/to/vibe-kanban)"; exit 1; }
export VK_DIR
TEAM_ROOT="$(cd "$VK_DIR/.." && pwd)"

echo "[build] source : $VK_DIR"

# 1) Ensure source exists.
if [ ! -f "$VK_DIR/Cargo.toml" ]; then
  echo "vibe-kanban source is missing Cargo.toml at: $VK_DIR" >&2
  exit 1
fi

# 2) Apply downstream patch stack when the source is a git checkout.
# In flattened/vendored deployments (no git metadata), skip patch re-application
# and build the already-patched tree as-is.
if git -C "$VK_DIR" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "[build] applying patch stack..."
  git -C "$VK_DIR" reset --hard >/dev/null 2>&1 || true
  git -C "$VK_DIR" clean -fd     >/dev/null 2>&1 || true
  "$TEAM_ROOT/scripts/apply-patches.sh" "$VK_DIR"
else
  echo "[build] source is not a git checkout; skipping patch re-application"
  echo "[build] assuming $VK_DIR already contains the desired patched files"
fi

# 3) Build via compose (tags match the compose `image:` field from .env).
echo "[build] building remote..."
docker compose build remote

echo "[build] ✅ done. Start the stack with:  docker compose up -d"
