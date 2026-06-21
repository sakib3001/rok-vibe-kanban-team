#!/usr/bin/env bash
#
# Build the linux-x64 `vibe-kanban` (server) binary so it runs on any glibc
# >= 2.31 host (Ubuntu 20.04 / 22.04 / 24.04). The native build in
# publish-npm.sh links against the build host's glibc; on Ubuntu 24.04 that is
# 2.39, which the dynamic loader rejects on 22.04/20.04. This script instead
# compiles inside Debian bullseye (glibc 2.31) via Docker.
#
# It produces, under the launcher's R2 staging dir:
#   r2-upload-rokfiles/binaries/<TAG>/linux-x64/vibe-kanban.zip
#   r2-upload-rokfiles/binaries/<TAG>/manifest.json   (sha256 + size)
#
# Upload to R2 separately (see the commands this script prints), then have
# clients clear ~/.vibe-kanban/bin/<TAG> so they re-download.
#
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VIBE_DIR="${ROOT_DIR}/vibe-kanban"
STAGE_DIR="${ROOT_DIR}/rok-vibe-kanban-launcher/r2-upload-rokfiles"
DOCKERFILE="${ROOT_DIR}/scripts/Dockerfile.portable-linux"
IMAGE_TAG="vk-server-portable:bullseye"
PLATFORM="linux-x64"

log() { printf '\033[1;34m[build-portable]\033[0m %s\n' "$*"; }
die() { printf '\033[1;31m[build-portable] ERROR:\033[0m %s\n' "$*" >&2; exit 1; }

command -v docker >/dev/null || die "docker is required."
[ -f "${VIBE_DIR}/Cargo.toml" ] || die "vibe-kanban source not found at ${VIBE_DIR} (init the submodule)."

# Binary tag uses the v-prefixed npm version of the vibe-kanban-team client.
VERSION="$(node -p "require('${VIBE_DIR}/package.json').version")"
TAG="${ROK_VK_BINARY_TAG:-v${VERSION}}"
OUT_DIR="${STAGE_DIR}/binaries/${TAG}/${PLATFORM}"
log "Building ${PLATFORM} server for tag ${TAG} (glibc 2.31 / bullseye)..."

DOCKER_BUILDKIT=1 docker build --target builder -f "${DOCKERFILE}" -t "${IMAGE_TAG}" "${VIBE_DIR}"

WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT
CID="$(docker create "${IMAGE_TAG}")"
docker cp "${CID}:/usr/local/bin/server" "${WORK}/vibe-kanban"
docker rm "${CID}" >/dev/null

# Fail loudly if the result is not broadly compatible.
MAXGLIBC="$(objdump -T "${WORK}/vibe-kanban" 2>/dev/null \
  | grep -oE 'GLIBC_[0-9]+\.[0-9]+(\.[0-9]+)?' | sort -uV | tail -1)"
log "Max required ${MAXGLIBC} (must be <= GLIBC_2.31 to cover Ubuntu 20.04+)"
case "${MAXGLIBC}" in
  GLIBC_2.1[0-9]|GLIBC_2.2[0-9]|GLIBC_2.30|GLIBC_2.31) : ;;
  *) die "Binary requires ${MAXGLIBC}; would not run on Ubuntu 20.04/22.04. Aborting." ;;
esac

mkdir -p "${OUT_DIR}"
( cd "${WORK}" && zip -j -q vibe-kanban.zip vibe-kanban )
cp "${WORK}/vibe-kanban.zip" "${OUT_DIR}/vibe-kanban.zip"

SHA="$(sha256sum "${OUT_DIR}/vibe-kanban.zip" | awk '{print $1}')"
SIZE="$(stat -c%s "${OUT_DIR}/vibe-kanban.zip")"

# Merge into the version manifest (preserve other platforms if present).
MANIFEST="${STAGE_DIR}/binaries/${TAG}/manifest.json"
node -e '
const fs = require("fs");
const [manifestPath, tag, platform, sha, size] = process.argv.slice(1);
let m = { version: tag, platforms: {} };
try { m = JSON.parse(fs.readFileSync(manifestPath, "utf8")); } catch {}
m.version = tag;
m.platforms = m.platforms || {};
m.platforms[platform] = m.platforms[platform] || {};
m.platforms[platform]["vibe-kanban"] = { sha256: sha, size: Number(size) };
fs.writeFileSync(manifestPath, JSON.stringify(m, null, 2) + "\n");
' "${MANIFEST}" "${TAG}" "${PLATFORM}" "${SHA}" "${SIZE}"

log "Staged: ${OUT_DIR}/vibe-kanban.zip"
log "  sha256=${SHA}"
log "  size=${SIZE}"
log "Updated manifest: ${MANIFEST}"
cat <<NOTE

Next — upload to R2 (needs R2_ENDPOINT / R2_BUCKET + AWS_* creds; e.g. source
scripts/publish-credentials.bashrc and export AWS_ACCESS_KEY_ID/SECRET from R2_*):

  aws --endpoint-url "\${R2_ENDPOINT}" s3 cp \\
    "${OUT_DIR}/vibe-kanban.zip" \\
    "s3://\${R2_BUCKET}/binaries/${TAG}/${PLATFORM}/vibe-kanban.zip"

  aws --endpoint-url "\${R2_ENDPOINT}" s3 cp \\
    "${MANIFEST}" \\
    "s3://\${R2_BUCKET}/binaries/${TAG}/manifest.json" \\
    --content-type application/json

Then on each client, clear the cache so it re-downloads the new binary:

  rm -rf ~/.vibe-kanban/bin/${TAG}
  systemctl --user restart vibe-kanban
NOTE
