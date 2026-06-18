#!/usr/bin/env bash
# Nightly release check for the shared vibe-kanban submodule.
#
# Detects new upstream tags, verifies patches apply cleanly, updates patch
# metadata and submodule refs, then commits + tags + pushes to trigger the
# existing tag-based CI pipelines.
#
# Usage:
#   scripts/nightly-check-release.sh frontend   # create NPM/frontend release tag
#   scripts/nightly-check-release.sh remote     # create remote/relay release tag
#
# Optional environment variables:
#   NIGHTLY_RELEASE_PUSH_TOKEN - GitHub token with contents:write to push commit/tag
#   DISCORD_WEBHOOK_URL        - Discord webhook URL for patch-failure alerts
#   DISCORD_WEBHOOK_PRODUCTION - Legacy webhook var name (fallback)
#   UPSTREAM_REPO_API          - GitHub API base for upstream (default: BloopAI/vibe-kanban)
#   DEFAULT_BRANCH             - Repo default branch (default: main)
#   DRY_RUN                    - set to "1" to skip push

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="${1:-}"
DEFAULT_BRANCH="${DEFAULT_BRANCH:-main}"
WEBHOOK_URL="${DISCORD_WEBHOOK_URL:-${DISCORD_WEBHOOK_PRODUCTION:-}}"

# ── Validate input ───────────────────────────────────────────────────────────
if [ -z "$TARGET" ] || { [ "$TARGET" != "frontend" ] && [ "$TARGET" != "remote" ]; }; then
  echo "Usage: $0 <frontend|remote>"
  exit 1
fi

# ── Configuration per target ─────────────────────────────────────────────────
UPSTREAM_REPO_API="${UPSTREAM_REPO_API:-https://api.github.com/repos/BloopAI/vibe-kanban}"
SUBMODULE_PATH="vibe-kanban"
UPSTREAM_TAG_REGEX='^v?[0-9]+\.[0-9]+\.[0-9]+([-.].*)?$'

if [ "$TARGET" = "frontend" ]; then
  LOCAL_TAG_REGEX='^v[0-9]+\.[0-9]+\.[0-9]+-[0-9]{14}$'
else
  LOCAL_TAG_REGEX='^remote-v?[0-9]+\.[0-9]+\.[0-9]+([-.].*)?$'
fi

echo "=== Nightly release check: $TARGET ==="
echo "Submodule: $SUBMODULE_PATH"
echo "Patch dir: patches/"

SERIES_FILE="${REPO_ROOT}/patches/series"

# ── Fetch upstream tags ──────────────────────────────────────────────────────
echo ""
echo "Fetching upstream tags..."

UPSTREAM_TAGS=$(curl -sf "${UPSTREAM_REPO_API}/git/refs/tags" \
  | jq -r '.[].ref' \
  | sed 's|^refs/tags/||' \
  | grep -E "$UPSTREAM_TAG_REGEX" \
  | sort -V)

if [ -z "$UPSTREAM_TAGS" ]; then
  echo "No upstream tags matched pattern: $UPSTREAM_TAG_REGEX"
  exit 0
fi

echo "Upstream tags found: $(echo "$UPSTREAM_TAGS" | wc -l)"

# ── Fetch our tags ───────────────────────────────────────────────────────────
echo "Fetching local tags..."
git fetch --tags origin 2>/dev/null || true

LOCAL_TAGS=$(git tag -l | grep -E "$LOCAL_TAG_REGEX" | sort -V || true)

echo "Local tags found: $(echo "$LOCAL_TAGS" | grep -c . || echo 0)"

# ── Find new upstream tags newer than the latest released upstream version ───
LATEST_RELEASED_UPSTREAM=""
if [ -n "$LOCAL_TAGS" ]; then
  LATEST_LOCAL=$(echo "$LOCAL_TAGS" | tail -1)
  echo "Latest local tag: $LATEST_LOCAL"
  if [ "$TARGET" = "frontend" ]; then
    LATEST_RELEASED_UPSTREAM="${LATEST_LOCAL%-*}"
  else
    LATEST_RELEASED_UPSTREAM="${LATEST_LOCAL#remote-}"
  fi
  echo "Latest released upstream tag: $LATEST_RELEASED_UPSTREAM"
fi

if [ -n "$LATEST_RELEASED_UPSTREAM" ]; then
  NEW_TAGS=$(printf '%s\n%s\n' "$LATEST_RELEASED_UPSTREAM" "$UPSTREAM_TAGS" \
    | sort -V -u \
    | awk -v latest="$LATEST_RELEASED_UPSTREAM" 'seen { print } $0 == latest { seen = 1 }') || true
else
  NEW_TAGS="$UPSTREAM_TAGS"
fi

if [ -z "$NEW_TAGS" ]; then
  echo ""
  echo "No new upstream tags. Nothing to do."
  exit 0
fi

echo ""
echo "New upstream tags detected:"
echo "$NEW_TAGS"

# Take only the latest (highest version)
LATEST_TAG=$(echo "$NEW_TAGS" | tail -1)
echo ""
echo "Processing latest upstream tag: $LATEST_TAG"

# ── Resolve the tag to a commit SHA ──────────────────────────────────────────
TAG_REF=$(curl -sf "${UPSTREAM_REPO_API}/git/refs/tags/${LATEST_TAG}")
TAG_OBJ_TYPE=$(echo "$TAG_REF" | jq -r '.object.type')
TAG_OBJ_SHA=$(echo "$TAG_REF" | jq -r '.object.sha')

# Dereference annotated tags to get the commit
if [ "$TAG_OBJ_TYPE" = "tag" ]; then
  COMMIT_SHA=$(curl -sf "${UPSTREAM_REPO_API}/git/tags/${TAG_OBJ_SHA}" | jq -r '.object.sha')
else
  COMMIT_SHA="$TAG_OBJ_SHA"
fi

echo "Tag $LATEST_TAG -> commit $COMMIT_SHA"

# ── Initialize and update submodule ──────────────────────────────────────────
echo ""
echo "Initializing submodule $SUBMODULE_PATH..."
git submodule update --init "$SUBMODULE_PATH"

echo "Checking out $COMMIT_SHA in $SUBMODULE_PATH..."
git -C "${REPO_ROOT}/${SUBMODULE_PATH}" fetch origin
git -C "${REPO_ROOT}/${SUBMODULE_PATH}" checkout "$COMMIT_SHA"

# ── Verify patches apply cleanly ────────────────────────────────────────────
echo ""
echo "Verifying patches apply cleanly..."

PATCH_FAILED=false
FAILED_PATCH=""

if [ -f "$SERIES_FILE" ]; then
  while IFS= read -r patch_name; do
    case "${patch_name}" in
      "" | \#*) continue ;;
    esac

    PATCH_FILE="${REPO_ROOT}/patches/${patch_name}"
    if [ ! -f "$PATCH_FILE" ]; then
      echo "  ERROR: Patch file missing: $patch_name"
      PATCH_FAILED=true
      FAILED_PATCH="$patch_name (file missing)"
      break
    fi

    if ! git -C "${REPO_ROOT}/${SUBMODULE_PATH}" apply --check --whitespace=nowarn "$PATCH_FILE" 2>/dev/null; then
      echo "  FAIL: ${patch_name} does not apply cleanly"
      PATCH_FAILED=true
      FAILED_PATCH="$patch_name"
      break
    fi

    echo "  OK: ${patch_name}"
  done < "$SERIES_FILE"
else
  echo "No series file at $SERIES_FILE"
fi

if [ "$PATCH_FAILED" = true ]; then
  echo ""
  echo "Patch verification FAILED for $TARGET at tag $LATEST_TAG"
  echo "Failed patch: $FAILED_PATCH"

  # ── Discord notification for patch failure ─────────────────────────────
  if [ -n "${WEBHOOK_URL:-}" ]; then
    echo "Sending Discord notification..."

    PAYLOAD=$(cat <<DISCORD_EOF
{
  "embeds": [{
    "title": "Patch Failure: ${TARGET}",
    "description": "Patches are outdated and need manual update.",
    "color": 15548997,
    "fields": [
      {"name": "Target", "value": "\`${TARGET}\`", "inline": true},
      {"name": "New Upstream Tag", "value": "\`${LATEST_TAG}\`", "inline": true},
      {"name": "Commit", "value": "\`${COMMIT_SHA:0:12}\`", "inline": true},
      {"name": "Failed Patch", "value": "\`${FAILED_PATCH}\`", "inline": false},
      {"name": "Action Required", "value": "Regenerate patches against the new upstream version and push updated patch files.", "inline": false}
    ],
    "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%S.000Z)",
    "footer": {"text": "Nightly Release Check"}
  }]
}
DISCORD_EOF
    )

    curl -sf -X POST \
      -H "Content-Type: application/json" \
      -d "$PAYLOAD" \
      "$WEBHOOK_URL" \
      --max-time 30 \
      --retry 3 \
      --retry-delay 5 || echo "Warning: Discord notification failed"
  fi

  exit 1
fi

echo ""
echo "All patches apply cleanly!"

# ── Update patch From-hash metadata ─────────────────────────────────────────
echo ""
echo "Updating patch compatibility metadata (From hash -> $COMMIT_SHA)..."

if [ -f "$SERIES_FILE" ]; then
  while IFS= read -r patch_name; do
    case "${patch_name}" in
      "" | \#*) continue ;;
    esac

    PATCH_FILE="${REPO_ROOT}/patches/${patch_name}"
    if [ -f "$PATCH_FILE" ]; then
      sed -i "1s/^From [0-9a-f]\\{40\\}/From ${COMMIT_SHA}/" "$PATCH_FILE"
      echo "  Updated: ${patch_name}"
    fi
  done < "$SERIES_FILE"
fi

# ── Stage changes ────────────────────────────────────────────────────────────
echo ""
echo "Staging submodule ref and patch updates..."

git -C "$REPO_ROOT" add "$SUBMODULE_PATH"
git -C "$REPO_ROOT" add "${REPO_ROOT}/patches/"

# Check if there are actual changes to commit
if git -C "$REPO_ROOT" diff --cached --quiet; then
  echo "No changes to commit (submodule already at this version?)"
  exit 0
fi

# ── Commit, tag, push ───────────────────────────────────────────────────────
if [ "$TARGET" = "frontend" ]; then
  RELEASE_TAG="${LATEST_TAG}-$(date -u +%Y%m%d%H%M%S)"
  COMMIT_MSG="chore: bump vibe-kanban to ${LATEST_TAG} for frontend release"
else
  RELEASE_TAG="remote-${LATEST_TAG#remote-}"
  COMMIT_MSG="chore: bump vibe-kanban to ${LATEST_TAG} for remote release"
fi

echo ""
echo "Committing: $COMMIT_MSG"
echo "Tag: $RELEASE_TAG"

# Configure git identity for CI
git -C "$REPO_ROOT" config user.email "nightly-bot@users.noreply.github.com"
git -C "$REPO_ROOT" config user.name "Nightly Release Bot"

git -C "$REPO_ROOT" commit -m "$COMMIT_MSG"
git -C "$REPO_ROOT" tag "$RELEASE_TAG"

if [ "${DRY_RUN:-}" = "1" ]; then
  echo ""
  echo "[DRY_RUN] Would push commit and tag $RELEASE_TAG"
  exit 0
fi

# Push using dedicated GitHub token so downstream push-tag workflows can run.
if [ -z "${NIGHTLY_RELEASE_PUSH_TOKEN:-}" ]; then
  echo "ERROR: NIGHTLY_RELEASE_PUSH_TOKEN not set — cannot push commit/tag"
  exit 1
fi

if [ -z "${GITHUB_REPOSITORY:-}" ]; then
  echo "ERROR: GITHUB_REPOSITORY is not set"
  exit 1
fi

PUSH_URL="https://x-access-token:${NIGHTLY_RELEASE_PUSH_TOKEN}@github.com/${GITHUB_REPOSITORY}.git"

echo "Pushing to origin..."
git -C "$REPO_ROOT" push "$PUSH_URL" "HEAD:${DEFAULT_BRANCH}"
git -C "$REPO_ROOT" push "$PUSH_URL" "$RELEASE_TAG"

echo ""
echo "=== Done! Pushed $RELEASE_TAG ==="
echo "The tag-based CI pipeline should now trigger automatically."
