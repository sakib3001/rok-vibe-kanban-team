#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BACKUP_DIR="${BACKUP_DIR:-$ROOT_DIR/backups}"
STAMP="$(date +%F_%H%M%S)"
OUT="$BACKUP_DIR/remote-$(date +%F).sql"

mkdir -p "$BACKUP_DIR"

cd "$ROOT_DIR"
docker compose exec -T postgres pg_dump -U remote remote > "$OUT"
gzip -f "$OUT"

echo "Backup written: ${OUT}.gz"
