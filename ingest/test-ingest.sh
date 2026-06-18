#!/usr/bin/env bash
# Exercise the ingestion API across scenarios.
#
# Usage:
#   INGEST_API_KEY=<key> ./test-ingest.sh                 # against prod
#   INGEST_API_KEY=<key> BASE=http://localhost ./test-ingest.sh   # against local stack
#
# Get the prod key:
#   ssh -i ~/.ssh/dev ubuntu@103.228.38.106 'grep INGEST_API_KEY /home/ubuntu/vibe-kanban/.env'
#
# NOTE: success cases create real issues (titled "[TEST] ...") in the target
# project. Clean them up afterwards — see the bottom of this file.
set -u

BASE="${BASE:-https://vk.rokomari.io}"
KEY="${INGEST_API_KEY:?Set INGEST_API_KEY}"
EP="$BASE/ingest/issues"
JSON=(-H 'content-type: application/json')
AUTH=(-H "X-API-Key: $KEY")

post() { # label, curl-args...
  local label="$1"; shift
  printf '\n=== %s ===\n' "$label"
  curl -s -o /tmp/ing.body -w "HTTP %{http_code}\n" -X POST "$EP" "$@"
  cat /tmp/ing.body; echo
}

# --- success cases ----------------------------------------------------------
post "1. minimal (title only) -> 201" "${AUTH[@]}" "${JSON[@]}" \
  -d '{"title":"[TEST] minimal"}'

post "2. full (description + priority + dedup) -> 201" "${AUTH[@]}" "${JSON[@]}" \
  -d '{"title":"[TEST] full","description":"detailed body","priority":"High","dedup_key":"test-dedup-A"}'

post "3. dedup repeat (same key) -> 200 deduped, same id" "${AUTH[@]}" "${JSON[@]}" \
  -d '{"title":"[TEST] should-not-create","dedup_key":"test-dedup-A"}'

for p in Urgent High Medium Low; do
  post "4. priority=$p -> 201" "${AUTH[@]}" "${JSON[@]}" \
    -d "{\"title\":\"[TEST] prio $p\",\"priority\":\"$p\"}"
done

# --- assignee cases (set ASSIGNEE=<member-email> to test the resolved path) --
if [ -n "${ASSIGNEE:-}" ]; then
  post "4a. assignee=$ASSIGNEE (member) -> 201, assignee.resolved:true" "${AUTH[@]}" "${JSON[@]}" \
    -d "{\"title\":\"[TEST] assigned\",\"assignee\":\"$ASSIGNEE\"}"
fi
post "4b. unknown assignee -> 201, assignee.resolved:false (issue still created)" "${AUTH[@]}" "${JSON[@]}" \
  -d '{"title":"[TEST] bad-assignee","assignee":"nobody@example.invalid"}'

# --- auth cases -------------------------------------------------------------
post "5. no API key -> 401" "${JSON[@]}" -d '{"title":"x"}'
post "6. wrong API key -> 401" -H "X-API-Key: wrong" "${JSON[@]}" -d '{"title":"x"}'
post "7. Authorization: Bearer (also accepted) -> 201" \
  -H "Authorization: Bearer $KEY" "${JSON[@]}" -d '{"title":"[TEST] bearer-auth"}'

# --- validation cases -------------------------------------------------------
post "8. missing title -> 400" "${AUTH[@]}" "${JSON[@]}" -d '{"description":"no title"}'
post "9. bad priority -> 400" "${AUTH[@]}" "${JSON[@]}" -d '{"title":"x","priority":"banana"}'
post "10. invalid JSON -> 400" "${AUTH[@]}" "${JSON[@]}" -d '{not json}'

# --- health (site up; /health routes to the remote, not ingest) -------------
printf '\n=== health ===\n'
curl -s -o /dev/null -w "GET ${BASE}/health -> HTTP %{http_code}\n" "$BASE/health"

cat <<'CLEANUP'

------------------------------------------------------------------------------
CLEANUP (removes the [TEST] issues this script created), run on the prod host:

  ssh -i ~/.ssh/dev ubuntu@103.228.38.106 'cd /home/ubuntu/vibe-kanban/docker && \
    docker compose exec -T postgres psql -U remote -d remote -c \
    "DELETE FROM issue_followers WHERE issue_id IN (SELECT id FROM issues WHERE title LIKE '"'"'[TEST]%'"'"'); \
     DELETE FROM issues WHERE title LIKE '"'"'[TEST]%'"'"';" && \
    docker compose exec -T ingest sh -c "rm -f /data/dedup.json" && \
    docker compose --profile ingest restart ingest'
------------------------------------------------------------------------------
CLEANUP
