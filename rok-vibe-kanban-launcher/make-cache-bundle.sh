#!/usr/bin/env bash
set -euo pipefail

# Creates a portable cache bundle for offline/cross-device installs.
# Output archive contains:
#   .vibe-kanban/bin/v<version>/<platform>/*
#   .vibe-kanban/desktop/v<version>/*   (if present)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT_FILE="${1:-}"

detect_platform_dir() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"

  case "$os" in
    Linux) os="linux" ;;
    Darwin) os="macos" ;;
    MINGW*|MSYS*|CYGWIN*) os="windows" ;;
    *) echo "Unsupported OS: ${os}" >&2; exit 1 ;;
  esac

  case "$arch" in
    x86_64|amd64) arch="x64" ;;
    aarch64|arm64) arch="arm64" ;;
    *) echo "Unsupported architecture: ${arch}" >&2; exit 1 ;;
  esac

  printf '%s-%s' "$os" "$arch"
}

VK_TEAM_VERSION="$(
  node -e "const p=require('${SCRIPT_DIR}/package.json');process.stdout.write(p.dependencies['vibe-kanban-team']);"
)"
VK_BINARY_TAG="v${VK_TEAM_VERSION}"
VK_PLATFORM_DIR="$(detect_platform_dir)"

if [ -z "$OUT_FILE" ]; then
  OUT_FILE="${SCRIPT_DIR}/bundles/rok-vk-cache-${VK_BINARY_TAG}-${VK_PLATFORM_DIR}.tar.gz"
fi

BIN_DIR="${HOME}/.vibe-kanban/bin/${VK_BINARY_TAG}/${VK_PLATFORM_DIR}"
DESKTOP_DIR="${HOME}/.vibe-kanban/desktop/${VK_BINARY_TAG}"

[ -d "$BIN_DIR" ] || {
  echo "Missing cache directory: $BIN_DIR" >&2
  echo "Run rok-vibe-kanban once on this machine before bundling." >&2
  exit 1
}

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

mkdir -p "${TMP_DIR}/.vibe-kanban/bin/${VK_BINARY_TAG}"
cp -a "$BIN_DIR" "${TMP_DIR}/.vibe-kanban/bin/${VK_BINARY_TAG}/"

if [ -d "$DESKTOP_DIR" ]; then
  mkdir -p "${TMP_DIR}/.vibe-kanban/desktop"
  cp -a "$DESKTOP_DIR" "${TMP_DIR}/.vibe-kanban/desktop/"
fi

mkdir -p "$(dirname "$OUT_FILE")"
tar -czf "$OUT_FILE" -C "$TMP_DIR" ".vibe-kanban"

echo "Created cache bundle: $OUT_FILE"
echo "Contains tag: $VK_BINARY_TAG"
echo "Platform: $VK_PLATFORM_DIR"
