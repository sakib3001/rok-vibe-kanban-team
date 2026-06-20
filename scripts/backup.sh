#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BACKUP_DIR="${BACKUP_DIR:-$ROOT_DIR/backups}"
STAMP="$(date +%F_%H%M%S)"
OUT="$BACKUP_DIR/remote-$(date +%F).sql"
MEMORY_OUT="$BACKUP_DIR/memory-$(date +%F).sql"

mkdir -p "$BACKUP_DIR"

cd "$ROOT_DIR"
docker compose exec -T postgres pg_dump -U remote remote > "$OUT"
gzip -f "$OUT"

echo "Backup written: ${OUT}.gz"

if docker compose ps --services --filter status=running | grep -qx "memory-db"; then
  docker compose exec -T memory-db pg_dump -U memory memory > "$MEMORY_OUT"
  gzip -f "$MEMORY_OUT"
  echo "Backup written: ${MEMORY_OUT}.gz"
else
  echo "memory-db is not running; skipping memory backup."
fi
