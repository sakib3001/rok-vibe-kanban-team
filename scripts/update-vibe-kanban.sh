#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/update-vibe-kanban.sh <tag-or-commit>

Examples:
  scripts/update-vibe-kanban.sh v1.4.0
  scripts/update-vibe-kanban.sh 3a088ff6f705900a8bb2ab29eade7bbf9f5bf76c
USAGE
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  usage
  exit 0
fi

TARGET="${1:-}"
if [ -z "${TARGET}" ]; then
  usage
  exit 1
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if ! git -C "${REPO_ROOT}/vibe-kanban" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "Submodule not initialized. Run: git submodule update --init --recursive"
  exit 1
fi

git -C "${REPO_ROOT}/vibe-kanban" fetch --tags origin
git -C "${REPO_ROOT}/vibe-kanban" checkout "${TARGET}"

git -C "${REPO_ROOT}" add vibe-kanban

echo "Updated shared vibe-kanban submodule to ${TARGET}."
echo "Next:"
echo "  ./scripts/apply-patches.sh"
echo "  git status"
echo "  git commit -m \"chore: bump vibe-kanban to ${TARGET}\""
echo "  git tag v${TARGET#v}-<timestamp>    # npm/frontend release"
echo "  git tag remote-v${TARGET#v}         # remote/relay release"
echo "  git push"
echo "Then push the release tag you need and deploy using scripts/deploy.sh <commit-sha>."
