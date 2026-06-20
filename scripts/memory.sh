#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ENV_FILE="${ROOT}/.env"

usage() {
  cat <<'USAGE'
Usage:
  scripts/memory.sh health
  scripts/memory.sh search "<query>" [org_id]
  scripts/memory.sh ingest-docs [org_id]
  scripts/memory.sh ingest-issues [org_id]
  scripts/memory.sh reembed

Reads MEMORY_API_KEY and PUBLIC_DOMAIN from .env by default.
USAGE
}

envf() { [ -f "$ENV_FILE" ] && grep -E "^$1=" "$ENV_FILE" | head -1 | cut -d= -f2- || true; }

BASE="${MEMORY_BASE:-https://$(envf PUBLIC_DOMAIN)}"
KEY="${MEMORY_API_KEY:-$(envf MEMORY_API_KEY)}"
ORG_DEFAULT="${MEMORY_DEFAULT_ORG_ID:-$(envf MEMORY_DEFAULT_ORG_ID)}"

cmd="${1:-}"; shift || true
[ -n "$cmd" ] || { usage; exit 2; }

api() {
  local path="$1" body="${2:-}"
  curl -fsS -X POST "${BASE}${path}" \
    -H "X-API-Key: ${KEY}" \
    -H 'content-type: application/json' \
    -d "${body}"
}

case "$cmd" in
  health)
    curl -fsS "${BASE}/health" | jq .
    if [ -n "$KEY" ] && [ -n "$ORG_DEFAULT" ]; then
      api "/memory/search" "{\"org_id\":\"${ORG_DEFAULT}\",\"query\":\"runbook\",\"top_k\":1}" | jq .
    fi
    ;;
  search)
    q="${1:-}"; org="${2:-$ORG_DEFAULT}"
    [ -n "$q" ] || { echo "query required" >&2; exit 2; }
    [ -n "$org" ] || { echo "org_id required (arg or MEMORY_DEFAULT_ORG_ID)" >&2; exit 2; }
    api "/memory/search" "$(printf '{"org_id":"%s","query":"%s","top_k":10}' "$org" "$q")" | jq .
    ;;
  ingest-docs)
    org="${1:-$ORG_DEFAULT}"
    [ -n "$org" ] || { echo "org_id required (arg or MEMORY_DEFAULT_ORG_ID)" >&2; exit 2; }
    api "/memory/ingest/docs" "{\"org_id\":\"${org}\"}" | jq .
    ;;
  ingest-issues)
    org="${1:-$ORG_DEFAULT}"
    [ -n "$org" ] || { echo "org_id required (arg or MEMORY_DEFAULT_ORG_ID)" >&2; exit 2; }
    api "/memory/ingest/issues" "{\"org_id\":\"${org}\"}" | jq .
    ;;
  reembed)
    api "/memory/reembed" '{}' | jq .
    ;;
  -h|--help|help)
    usage
    ;;
  *)
    echo "unknown command: $cmd" >&2
    usage
    exit 2
    ;;
esac
