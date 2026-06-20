#!/usr/bin/env bash
#
# Thorough E2E acceptance test for the Organization Memory layer.
# Run on the server that hosts the stack (reads .env). Brings up the `memory`
# profile, exercises acceptance criteria A1-A6 + the hardening fixes, and prints
# a PASS/FAIL summary. Safe + idempotent; restores any docs it mutates.
#
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ENV_FILE="${ROOT}/.env"
cd "$ROOT"

envf() { [ -f "$ENV_FILE" ] && grep -E "^$1=" "$ENV_FILE" | head -1 | cut -d= -f2- || true; }

# Server (default) vs local: override MEMORY_DC + MEMORY_BASE to point at the
# standalone local compose (docker-compose.memory-local.yml) on a dev machine.
DC="${MEMORY_DC:-docker compose --profile memory}"
BASE="${MEMORY_BASE:-https://$(envf PUBLIC_DOMAIN)}"
KEY="${MEMORY_API_KEY:-$(envf MEMORY_API_KEY)}"
ORG_ID="${MEMORY_DEFAULT_ORG_ID:-$(envf MEMORY_DEFAULT_ORG_ID)}"; [ -n "$ORG_ID" ] || ORG_ID="$(envf INGEST_ORG_ID)"
EMBED_MODEL_NAME="${EMBED_MODEL:-$(envf EMBED_MODEL)}"; [ -n "$EMBED_MODEL_NAME" ] || EMBED_MODEL_NAME="nomic-embed-text"
TEST_DETERMINISTIC="${TEST_DETERMINISTIC:-1}"   # set 0 to skip live VK tools (local)

[ -n "$KEY" ]    || { echo "MEMORY_API_KEY missing (.env or env)" >&2; exit 1; }
[ -n "$ORG_ID" ] || { echo "org id missing (MEMORY_DEFAULT_ORG_ID/INGEST_ORG_ID)" >&2; exit 1; }

PASS=0; FAIL=0
check() { # name  condition(0=pass)
  if [ "$2" -eq 0 ]; then echo "  PASS  $1"; PASS=$((PASS+1)); else echo "  FAIL  $1"; FAIL=$((FAIL+1)); fi
}
api() { curl -fsS -X POST "${BASE}$1" -H "X-API-Key: ${KEY}" -H 'content-type: application/json' -d "$2"; }
code() { curl -s -o /dev/null -w '%{http_code}' -X POST "${BASE}$1" -H "X-API-Key: ${KEY}" -H 'content-type: application/json' -d "$2"; }
psqlm() { $DC exec -T memory-db psql -U memory -d memory -At -c "$1" </dev/null; }

echo "== A1: bring up memory profile (additive) =="
$DC up -d memory-db embedder memory >/dev/null 2>&1
$DC ps --services --filter status=running 2>/dev/null | grep -qx memory; check "memory profile running (additive, central stack untouched)" $?

echo "== embedder: ensure model present =="
$DC exec -T embedder ollama pull "$EMBED_MODEL_NAME" >/dev/null 2>&1
$DC exec -T embedder ollama list 2>/dev/null | grep -q "$EMBED_MODEL_NAME"; check "embed model '${EMBED_MODEL_NAME}' present" $?

echo "== health (via Caddy /memory/health) =="
curl -fsS "${BASE}/memory/health" | jq -e '.status=="ok"' >/dev/null 2>&1; check "/memory/health ok" $?

echo "== ingest docs (run 1) =="
R1="$(api /memory/ingest/docs "{\"org_id\":\"${ORG_ID}\"}")"; echo "    $R1"
echo "$R1" | jq -e '.ingested >= 1' >/dev/null 2>&1; check "docs ingested >= 1" $?
echo "$R1" | jq -e '.degraded == false' >/dev/null 2>&1; check "ingest used embeddings (degraded=false)" $?

echo "== ingest docs (run 2) — idempotency (gap #4) =="
R2="$(api /memory/ingest/docs "{\"org_id\":\"${ORG_ID}\"}")"; echo "    $R2"
echo "$R2" | jq -e '.ingested == 0 and .skipped >= 1' >/dev/null 2>&1; check "re-run skips unchanged (ingested=0, skipped>0)" $?

echo "== A3: semantic search + citations =="
S="$(api /memory/search "{\"org_id\":\"${ORG_ID}\",\"query\":\"runbook deployment backup\",\"top_k\":5}")"
echo "$S" | jq -e '.results | length >= 1' >/dev/null 2>&1; check "search returns results" $?
echo "$S" | jq -e '.degraded == false' >/dev/null 2>&1; check "search is semantic (degraded=false)" $?
echo "$S" | jq -e '.results[0].source_ref | test("^doc:")' >/dev/null 2>&1; check "results carry source_ref citation" $?

echo "== A3: cross-org isolation =="
XC="$(code /memory/search "{\"org_id\":\"00000000-0000-0000-0000-000000000000\",\"query\":\"anything\"}")"
check "foreign org_id rejected (got HTTP ${XC}, expect 403)" "$([ "$XC" = 403 ] && echo 0 || echo 1)"

echo "== A5: secret redaction =="
DOC=""; for d in HANDOFF.md SYSTEM_DESIGN.md GO_LIVE.md DEPLOYMENT_README.md; do [ -f "$d" ] && { DOC="$d"; break; }; done
if [ -n "$DOC" ]; then
  cp -a "$DOC" "/tmp/${DOC}.orig.$$"
  printf '\n\nTEST SECRET AKIAIOSFODNN7EXAMPLE and ghp_abcdefghijklmnopqrstuvwx0123456789\n' >> "$DOC"
  api /memory/ingest/docs "{\"org_id\":\"${ORG_ID}\"}" >/dev/null
  RAWHITS="$(psqlm "SELECT count(*) FROM memory_records WHERE org_id='${ORG_ID}' AND source_ref LIKE 'doc:${DOC}%' AND (content LIKE '%AKIAIOSFODNN7EXAMPLE%' OR content LIKE '%ghp_abcdefghij%')")"
  REDHITS="$(psqlm "SELECT count(*) FROM memory_records WHERE org_id='${ORG_ID}' AND source_ref LIKE 'doc:${DOC}%' AND content LIKE '%REDACTED%'")"
  check "raw secrets NOT stored (count=${RAWHITS})" "$([ "${RAWHITS:-1}" = 0 ] && echo 0 || echo 1)"
  check "redaction marker present (count=${REDHITS})" "$([ "${REDHITS:-0}" -ge 1 ] && echo 0 || echo 1)"
  cp -a "/tmp/${DOC}.orig.$$" "$DOC"; rm -f "/tmp/${DOC}.orig.$$"
  api /memory/ingest/docs "{\"org_id\":\"${ORG_ID}\"}" >/dev/null   # restore clean content
else
  echo "  SKIP  redaction ($DOC not found)"
fi

echo "== A4: degraded keyword fallback (embedder down) =="
$DC stop embedder >/dev/null 2>&1
D="$(api /memory/search "{\"org_id\":\"${ORG_ID}\",\"query\":\"deployment\",\"top_k\":5}")"
echo "$D" | jq -e '.degraded == true' >/dev/null 2>&1; check "search degrades to keyword (degraded=true)" $?
echo "$D" | jq -e '.results | length >= 1' >/dev/null 2>&1; check "keyword fallback still returns results" $?
$DC start embedder >/dev/null 2>&1
for _ in $(seq 1 20); do [ "$($DC exec -T embedder ollama list >/dev/null 2>&1 && echo ok)" = ok ] && break; sleep 3; done

echo "== reembed (heal null rows) =="
RE="$(api /memory/reembed '{"only_missing":true}')"; echo "    $RE"
echo "$RE" | jq -e 'has("reembedded")' >/dev/null 2>&1; check "reembed only_missing ok" $?

echo "== retention sweep (§7) =="
RT="$(api /memory/retention '{}')"; echo "    $RT"
echo "$RT" | jq -e 'has("soft_deleted") and has("purged")' >/dev/null 2>&1; check "retention sweep ok" $?

if [ "$TEST_DETERMINISTIC" != 1 ]; then
  echo "== A6: deterministic tools SKIPPED (TEST_DETERMINISTIC=0; needs live VK API) =="
else
echo "== A6: deterministic tools (live VK) =="
api /memory/recent_changes "{\"org_id\":\"${ORG_ID}\",\"since\":\"$(date -u -d '7 days ago' +%Y-%m-%dT%H:%M:%SZ)\",\"limit\":20}" | jq -e 'has("changes")' >/dev/null 2>&1; check "recent_changes ok" $?
PROJ="$(api /memory/recent_changes "{\"org_id\":\"${ORG_ID}\",\"since\":\"1970-01-01T00:00:00Z\",\"limit\":1}" | jq -r '.changes[0].project_id // empty')"
if [ -n "$PROJ" ]; then
  api /memory/project_brief "{\"project_id\":\"${PROJ}\"}" | jq -e '.project and .metrics' >/dev/null 2>&1; check "project_brief ok" $?
fi
fi

echo
echo "==================  RESULT: ${PASS} passed, ${FAIL} failed  =================="
[ "$FAIL" -eq 0 ]
