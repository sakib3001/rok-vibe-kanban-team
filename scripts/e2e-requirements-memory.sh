#!/usr/bin/env bash
#
# Local end-to-end validation for:
#   ingest upload -> draft -> approve -> publish -> memory ingest -> memory search
#
# This script runs isolated mock services for remote/r2/embedder, plus a real
# pgvector container and local ingest+memory node processes.
#
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

for cmd in docker node python3 curl; do
  command -v "$cmd" >/dev/null 2>&1 || {
    echo "Missing required command: $cmd" >&2
    exit 1
  }
done

ORG_ID="${ORG_ID:-11111111-1111-4111-8111-111111111111}"
PROJECT_ID="${PROJECT_ID:-22222222-2222-4222-8222-222222222222}"

PG_PORT="${PG_PORT:-55440}"
MOCK_EMBED_PORT="${MOCK_EMBED_PORT:-28136}"
MOCK_REMOTE_PORT="${MOCK_REMOTE_PORT:-29110}"
MOCK_R2_PORT="${MOCK_R2_PORT:-29120}"
MEMORY_PORT="${MEMORY_PORT:-28110}"
INGEST_PORT="${INGEST_PORT:-28111}"

PG_CONTAINER="memory-pg-e2e-${RANDOM}-$$"
TMP_REQ_FILE="$(mktemp /tmp/req-e2e-XXXXXX.md)"
STATE_FILE="$(mktemp /tmp/ingest-e2e-state-XXXXXX.json)"

cleanup() {
  set +e
  [ -n "${INGEST_PID:-}" ] && kill "$INGEST_PID" >/dev/null 2>&1
  [ -n "${MEMORY_PID:-}" ] && kill "$MEMORY_PID" >/dev/null 2>&1
  [ -n "${MOCK_REMOTE_PID:-}" ] && kill "$MOCK_REMOTE_PID" >/dev/null 2>&1
  [ -n "${MOCK_R2_PID:-}" ] && kill "$MOCK_R2_PID" >/dev/null 2>&1
  [ -n "${MOCK_EMBED_PID:-}" ] && kill "$MOCK_EMBED_PID" >/dev/null 2>&1
  [ -n "${INGEST_PID:-}" ] && wait "$INGEST_PID" >/dev/null 2>&1
  [ -n "${MEMORY_PID:-}" ] && wait "$MEMORY_PID" >/dev/null 2>&1
  [ -n "${MOCK_REMOTE_PID:-}" ] && wait "$MOCK_REMOTE_PID" >/dev/null 2>&1
  [ -n "${MOCK_R2_PID:-}" ] && wait "$MOCK_R2_PID" >/dev/null 2>&1
  [ -n "${MOCK_EMBED_PID:-}" ] && wait "$MOCK_EMBED_PID" >/dev/null 2>&1
  docker rm -f "$PG_CONTAINER" >/dev/null 2>&1
  rm -f "$TMP_REQ_FILE" "$STATE_FILE"
}
trap cleanup EXIT

if [ ! -d "$ROOT/ingest/node_modules" ]; then
  (cd "$ROOT/ingest" && npm ci >/dev/null)
fi
if [ ! -d "$ROOT/memory/node_modules" ]; then
  (cd "$ROOT/memory" && npm ci >/dev/null)
fi

docker run -d \
  --name "$PG_CONTAINER" \
  -e POSTGRES_USER=memory \
  -e POSTGRES_PASSWORD=pass \
  -e POSTGRES_DB=memory \
  -p "${PG_PORT}:5432" \
  pgvector/pgvector:pg16 >/dev/null

for _ in $(seq 1 60); do
  if docker exec "$PG_CONTAINER" pg_isready -U memory -d memory >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

MOCK_EMBED_PORT="$MOCK_EMBED_PORT" node - <<'NODE' >/tmp/mock-embed-e2e.log 2>&1 &
const http = require('http');
const port = Number(process.env.MOCK_EMBED_PORT);
http.createServer(async (req, res) => {
  if (req.method === 'POST' && req.url === '/embed') {
    const chunks = [];
    req.on('data', (d) => chunks.push(d));
    req.on('end', () => {
      const body = JSON.parse(Buffer.concat(chunks).toString('utf8') || '{}');
      const input = Array.isArray(body.input) ? body.input : [];
      const embeddings = input.map(() => [0.11, 0.22, 0.33, 0.44]);
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ embeddings }));
    });
    return;
  }
  res.writeHead(404);
  res.end('not found');
}).listen(port);
NODE
MOCK_EMBED_PID=$!

MOCK_REMOTE_PORT="$MOCK_REMOTE_PORT" node - <<'NODE' >/tmp/mock-remote-e2e.log 2>&1 &
const http = require('http');
const { URL } = require('url');
const port = Number(process.env.MOCK_REMOTE_PORT);
let n = 0;
const issues = new Map();
const read = (req) => new Promise((resolve, reject) => {
  const chunks = [];
  req.on('data', (d) => chunks.push(d));
  req.on('end', () => {
    const text = Buffer.concat(chunks).toString('utf8');
    if (!text) return resolve({});
    try { resolve(JSON.parse(text)); } catch { resolve({}); }
  });
  req.on('error', reject);
});
const send = (res, code, obj) => {
  res.writeHead(code, { 'content-type': 'application/json' });
  res.end(JSON.stringify(obj));
};
http.createServer(async (req, res) => {
  const u = new URL(req.url, 'http://x');
  if (req.method === 'POST' && u.pathname === '/v1/auth/local/login') {
    return send(res, 200, { access_token: 'tok', refresh_token: 'ref' });
  }
  if (req.method === 'POST' && u.pathname === '/v1/tokens/refresh') {
    return send(res, 200, { access_token: 'tok', refresh_token: 'ref' });
  }
  if (req.method === 'GET' && u.pathname === '/v1/project_statuses') {
    return send(res, 200, { project_statuses: [{ id: 'status-1', name: 'To do', sort_order: 1 }] });
  }
  if (req.method === 'POST' && u.pathname === '/v1/issues') {
    const body = await read(req);
    n += 1;
    const id = `issue-${n}`;
    issues.set(id, { id, ...body });
    return send(res, 200, { data: { id } });
  }
  if (req.method === 'POST' && u.pathname === '/v1/issues/bulk') {
    const body = await read(req);
    const out = [];
    for (const upd of (body.updates || [])) {
      const cur = issues.get(upd.id) || { id: upd.id };
      const next = { ...cur, ...upd };
      issues.set(upd.id, next);
      out.push(next);
    }
    return send(res, 200, { data: out, txid: 1 });
  }
  if (req.method === 'GET' && u.pathname.startsWith('/v1/issues/')) {
    const id = u.pathname.split('/').pop();
    const issue = issues.get(id);
    if (!issue) return send(res, 404, { error: 'not found' });
    return send(res, 200, { data: issue });
  }
  if (req.method === 'POST' && u.pathname === '/v1/issue_assignees') {
    return send(res, 200, { ok: true });
  }
  return send(res, 404, { error: 'not found', path: u.pathname });
}).listen(port);
NODE
MOCK_REMOTE_PID=$!

MOCK_R2_PORT="$MOCK_R2_PORT" node - <<'NODE' >/tmp/mock-r2-e2e.log 2>&1 &
const http = require('http');
const { URL } = require('url');
const port = Number(process.env.MOCK_R2_PORT);
const store = new Map();
http.createServer((req, res) => {
  const u = new URL(req.url, 'http://x');
  if (req.method === 'PUT') {
    const chunks = [];
    req.on('data', (d) => chunks.push(d));
    req.on('end', () => {
      store.set(u.pathname, Buffer.concat(chunks));
      res.writeHead(200);
      res.end('ok');
    });
    return;
  }
  if (req.method === 'GET') {
    if (!store.has(u.pathname)) {
      res.writeHead(404);
      res.end('missing');
      return;
    }
    res.writeHead(200);
    res.end(store.get(u.pathname));
    return;
  }
  res.writeHead(200);
  res.end('ok');
}).listen(port);
NODE
MOCK_R2_PID=$!

MEMORY_PORT="$MEMORY_PORT" \
MEMORY_API_KEY=mem-key \
MEMORY_DATABASE_URL="postgres://memory:pass@127.0.0.1:${PG_PORT}/memory" \
REMOTE_URL="http://127.0.0.1:${MOCK_REMOTE_PORT}" \
MEMORY_SVC_EMAIL=bot@example.com \
MEMORY_SVC_PASSWORD=secret \
MEMORY_DEFAULT_ORG_ID="$ORG_ID" \
EMBED_PROVIDER=http \
EMBED_BASE_URL="http://127.0.0.1:${MOCK_EMBED_PORT}" \
EMBED_MODEL=test-model \
EMBED_DIMENSIONS=4 \
EMBED_BATCH=8 \
node "$ROOT/memory/server.js" >/tmp/memory-e2e.log 2>&1 &
MEMORY_PID=$!

INGEST_PORT="$INGEST_PORT" \
INGEST_API_KEY=test-key \
INGEST_SVC_EMAIL=bot@example.com \
INGEST_SVC_PASSWORD=secret \
INGEST_PROJECT_ID="$PROJECT_ID" \
INGEST_ORG_ID="$ORG_ID" \
INGEST_REQUIREMENTS_FILE="$STATE_FILE" \
REMOTE_URL="http://127.0.0.1:${MOCK_REMOTE_PORT}" \
R2_ACCESS_KEY_ID=AKIATEST \
R2_SECRET_ACCESS_KEY=secret123 \
R2_REVIEW_ENDPOINT="http://127.0.0.1:${MOCK_R2_PORT}" \
R2_REVIEW_BUCKET=req-bucket \
R2_REGION=auto \
R2_PRESIGN_EXPIRY_SECS=300 \
INGEST_MEMORY_URL="http://127.0.0.1:${MEMORY_PORT}" \
INGEST_MEMORY_API_KEY=mem-key \
node "$ROOT/ingest/server.js" >/tmp/ingest-e2e.log 2>&1 &
INGEST_PID=$!

for _ in $(seq 1 30); do
  if curl -fsS "http://127.0.0.1:${MEMORY_PORT}/health" >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

printf '# Checkout Policy\n\nSpecialPolicyAlpha must be validated for premium orders.\n' >"$TMP_REQ_FILE"

UPLOAD_RESP="$(
  curl -fsS -X POST "http://127.0.0.1:${INGEST_PORT}/ingest/requirements/sources/upload" \
    -H "X-API-Key: test-key" \
    -F "file=@${TMP_REQ_FILE}" \
    -F "prefix=requirements/srs"
)"
OBJECT_KEY="$(python3 -c 'import json,sys; print(json.loads(sys.stdin.read())["object_key"])' <<<"$UPLOAD_RESP")"

CREATE_PAYLOAD="$(python3 -c 'import json,sys; key=sys.argv[1]; print(json.dumps({
  "source":{"type":"markdown","fingerprint":"checkout-req-memory","object_keys":[key]},
  "epic":{"title":"Checkout Req","summary":"Summary for memory","acceptance_criteria":["AC1","AC2"]},
  "child_tasks":[{"title":"Task A","objective":"Implement validator","acceptance_criteria":["Validator done"]}]
}))' "$OBJECT_KEY")"

CREATE_RESP="$(
  curl -fsS -X POST "http://127.0.0.1:${INGEST_PORT}/ingest/requirements/drafts" \
    -H "X-API-Key: test-key" \
    -H "Content-Type: application/json" \
    -d "$CREATE_PAYLOAD"
)"
DRAFT_ID="$(python3 -c 'import json,sys; print(json.loads(sys.stdin.read())["draft_id"])' <<<"$CREATE_RESP")"

APPROVE_RESP="$(
  curl -fsS -X POST "http://127.0.0.1:${INGEST_PORT}/ingest/requirements/drafts/${DRAFT_ID}/approve" \
    -H "X-API-Key: test-key" \
    -H "Content-Type: application/json" \
    -d '{"approved_by":"lead@rokomari.com"}'
)"

SEARCH_RESP="$(
  curl -fsS -X POST "http://127.0.0.1:${MEMORY_PORT}/memory/search" \
    -H "X-API-Key: mem-key" \
    -H "Content-Type: application/json" \
    -d "{\"org_id\":\"${ORG_ID}\",\"query\":\"SpecialPolicyAlpha premium orders\",\"top_k\":5}"
)"

APPROVE="$APPROVE_RESP" SEARCH="$SEARCH_RESP" python3 - <<'PY'
import json, os
a = json.loads(os.environ["APPROVE"])
s = json.loads(os.environ["SEARCH"])
assert a.get("approved") is True
mi = a.get("memory_ingest", {})
assert mi.get("ok") is True
body = mi.get("body", {})
assert body.get("ingested", 0) >= 2
results = s.get("results", [])
assert len(results) >= 1
joined = "\n".join(f'{r.get("summary","")} {r.get("source_ref","")}' for r in results)
assert "checkout-req-memory" in joined
print(json.dumps({
  "ok": True,
  "memory_ingested": body.get("ingested"),
  "memory_skipped": body.get("skipped"),
  "top_source_ref": results[0].get("source_ref"),
  "top_score": results[0].get("score"),
}))
PY

echo "E2E requirements->memory flow passed."
