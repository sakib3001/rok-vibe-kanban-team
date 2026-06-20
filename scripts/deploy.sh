#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/deploy.sh [options]

Docker Compose deployment helper for the root-level stack.

Options:
  --build               Build remote image from local source (runs ./build.sh)
  --pull                Pull images before rollout
  --profile-ingest      Include ingest profile during pull/up/ps
  --skip-backup         Skip pre-deploy postgres backup
  -h, --help            Show this help

Examples:
  ./scripts/deploy.sh --build --profile-ingest
  ./scripts/deploy.sh --pull
  ./scripts/deploy.sh --pull --profile-ingest --skip-backup
USAGE
}

DO_BUILD=0
DO_PULL=0
USE_INGEST_PROFILE=0
SKIP_BACKUP=0

while [ $# -gt 0 ]; do
  case "$1" in
    --build) DO_BUILD=1 ;;
    --pull) DO_PULL=1 ;;
    --profile-ingest) USE_INGEST_PROFILE=1 ;;
    --skip-backup) SKIP_BACKUP=1 ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      usage
      exit 2
      ;;
  esac
  shift
done

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

[ -f .env ] || {
  echo "No .env found in $ROOT (copy .env.example first)." >&2
  exit 1
}

envf() { grep -E "^$1=" .env | head -1 | cut -d= -f2- || true; }
PUBLIC_DOMAIN="${PUBLIC_DOMAIN:-$(envf PUBLIC_DOMAIN)}"
BACKUP_DIR="${BACKUP_DIR:-$ROOT/backups}"
PROFILE_ARGS=()
if [ "$USE_INGEST_PROFILE" -eq 1 ]; then
  PROFILE_ARGS=(--profile ingest)
fi

if [ "$SKIP_BACKUP" -ne 1 ]; then
  echo "[deploy] Creating pre-deploy DB backup..."
  mkdir -p "$BACKUP_DIR"
  BACKUP_FILE="$BACKUP_DIR/remote-$(date +%F-%H%M%S).sql.gz"
  docker compose "${PROFILE_ARGS[@]}" exec -T postgres pg_dump -U remote remote | gzip > "$BACKUP_FILE"
  echo "[deploy] Backup saved to: $BACKUP_FILE"
fi

if [ "$DO_BUILD" -eq 1 ]; then
  echo "[deploy] Building remote image from source..."
  ./build.sh
fi

if [ "$DO_PULL" -eq 1 ]; then
  echo "[deploy] Pulling images..."
  docker compose "${PROFILE_ARGS[@]}" pull
fi

echo "[deploy] Rolling out services..."
docker compose "${PROFILE_ARGS[@]}" up -d

echo "[deploy] Waiting for remote health..."
for _ in $(seq 1 30); do
  if docker compose "${PROFILE_ARGS[@]}" exec -T remote wget -qO- "http://localhost:8081/health" >/dev/null 2>&1; then
    break
  fi
  sleep 2
done

echo "[deploy] Service status:"
docker compose "${PROFILE_ARGS[@]}" ps

if [ -n "$PUBLIC_DOMAIN" ] && [ "$PUBLIC_DOMAIN" != "localhost" ]; then
  echo "[deploy] Public health check:"
  if curl -fsS "https://$PUBLIC_DOMAIN/v1/health" >/dev/null; then
    echo "[deploy] ✅ https://$PUBLIC_DOMAIN/v1/health is healthy"
  else
    echo "[deploy] ⚠️ public health check failed (check Caddy/remote logs)" >&2
  fi
fi
