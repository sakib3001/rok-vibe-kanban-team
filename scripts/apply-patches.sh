#!/bin/sh
set -eu

usage() {
  cat <<'USAGE'
Usage: scripts/apply-patches.sh [TARGET_REPO]

Applies patches to the shared Vibe Kanban checkout.

Arguments:
  TARGET_REPO        Path to checkout
                     Default: vibe-kanban

Patch Source:
  patches/series

Examples:
  ./scripts/apply-patches.sh
  ./scripts/apply-patches.sh vibe-kanban
  ./scripts/apply-patches.sh /path/to/vibe-kanban

USAGE
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  usage
  exit 0
fi

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET_REPO_INPUT="${1:-vibe-kanban}"

case "${TARGET_REPO_INPUT}" in
  /*) TARGET_REPO="${TARGET_REPO_INPUT}" ;;
  *) TARGET_REPO="${REPO_ROOT}/${TARGET_REPO_INPUT}" ;;
esac

TARGET_NAME="$(basename "${TARGET_REPO}")"

# Validate target repository
if ! git -C "${TARGET_REPO}" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "Error: Target repo not found or not a git repo: ${TARGET_REPO}"
  exit 1
fi

echo "Applying linear patch series to ${TARGET_NAME}"

apply_patch_series() {
  local series_file="$1"
  local patch_dir="$2"

  if [ ! -f "${series_file}" ]; then
    echo "Warning: Series file not found: ${series_file}"
    return 0
  fi

  local applied=0
  while IFS= read -r patch; do
    # Skip empty lines and comments
    case "${patch}" in
      ""|\#*) continue ;;
    esac

    PATCH_PATH="${patch_dir}/${patch}"
    if [ ! -f "${PATCH_PATH}" ]; then
      echo "Error: Patch not found: ${PATCH_PATH}"
      exit 1
    fi

    echo "  Applying: ${patch}"
    if ! git -C "${TARGET_REPO}" apply --whitespace=nowarn --3way "${PATCH_PATH}"; then
      echo "Error: Failed to apply patch: ${patch}"
      echo "You may need to resolve conflicts manually or update the patch for the current version."
      exit 1
    fi
    applied=$((applied + 1))
  done < "${series_file}"

  if [ "${applied}" -gt 0 ]; then
    echo "Applied ${applied} patch(es)"
  fi
}

SERIES_FILE="${REPO_ROOT}/patches/series"
PATCH_DIR="${REPO_ROOT}/patches"
apply_patch_series "${SERIES_FILE}" "${PATCH_DIR}"

echo "Patch application complete for ${TARGET_NAME}"
