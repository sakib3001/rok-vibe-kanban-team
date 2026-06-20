'use strict';
// Vibe Kanban issue-ingestion sidecar.
// POST /ingest/issues  -> creates an issue on the central server via /v1/issues,
// authenticating as a service account (self-host local auth). API-key guarded.
//
// Required env: INGEST_API_KEY, INGEST_SVC_EMAIL, INGEST_SVC_PASSWORD, INGEST_PROJECT_ID
// Optional env: INGEST_PORT(8090), REMOTE_URL(http://remote:8081), INGEST_ORG_ID,
//               INGEST_STATUS_ID, INGEST_STATUS_NAME(todo), INGEST_DEDUP_FILE(/data/dedup.json),
//               INGEST_DEDUP_TTL_DAYS(30), INGEST_PUBLIC_URL

const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');

const PORT = parseInt(process.env.INGEST_PORT || '8090', 10);
const REMOTE_URL = (process.env.REMOTE_URL || 'http://remote:8081').replace(/\/+$/, '');
const API_KEY = process.env.INGEST_API_KEY || '';
const SVC_EMAIL = process.env.INGEST_SVC_EMAIL || '';
const SVC_PASSWORD = process.env.INGEST_SVC_PASSWORD || '';
const PROJECT_ID = process.env.INGEST_PROJECT_ID || '';
// Team org id — only needed to resolve `assignee` emails. Optional.
const ORG_ID = process.env.INGEST_ORG_ID || '';
const STATUS_NAME_HINT = (process.env.INGEST_STATUS_NAME || 'todo').toLowerCase();
const DEDUP_FILE = process.env.INGEST_DEDUP_FILE || '/data/dedup.json';
const DEDUP_TTL_MS = (parseInt(process.env.INGEST_DEDUP_TTL_DAYS || '30', 10) || 30) * 24 * 3600 * 1000;
const DEDUP_MAX = 10000;
const STATUS_TTL_MS = 10 * 60 * 1000;
const MEMBER_TTL_MS = 5 * 60 * 1000;
const PUBLIC_URL = (process.env.INGEST_PUBLIC_URL || '').replace(/\/+$/, '');
// Server expects lowercase enum variants: urgent | high | medium | low.
const PRIORITIES = { urgent: 'urgent', high: 'high', medium: 'medium', low: 'low' };

// ---- startup validation ----------------------------------------------------
for (const [k, v] of Object.entries({
  INGEST_API_KEY: API_KEY,
  INGEST_SVC_EMAIL: SVC_EMAIL,
  INGEST_SVC_PASSWORD: SVC_PASSWORD,
  INGEST_PROJECT_ID: PROJECT_ID,
})) {
  if (!v) {
    console.error(`[ingest] FATAL: ${k} is required`);
    process.exit(1);
  }
}

// ---- state -----------------------------------------------------------------
let tokens = { access: null, refresh: null };
// Status: env-pinned (never expires) or name-resolved (TTL).
let statusId = process.env.INGEST_STATUS_ID || null;
let statusAt = statusId ? Infinity : 0;

// Dedup entries: { id, at }. Loaded from disk; TTL + capped to bound growth.
let dedup = {};
try {
  dedup = JSON.parse(fs.readFileSync(DEDUP_FILE, 'utf8')) || {};
} catch {
  dedup = {};
}
const inFlight = new Set(); // dedup_keys currently being created (race guard)

function persistDedup() {
  try {
    fs.mkdirSync(path.dirname(DEDUP_FILE), { recursive: true });
    fs.writeFileSync(DEDUP_FILE, JSON.stringify(dedup));
  } catch (e) {
    console.error(`[ingest] WARN: could not persist dedup store: ${e.message}`);
  }
}
function dedupGet(key) {
  const e = dedup[key];
  if (!e) return null;
  // Entries without `at` predate TTL support — treat as fresh once, then re-stamp.
  if (e.at && Date.now() - e.at > DEDUP_TTL_MS) {
    delete dedup[key];
    return null;
  }
  return e;
}
function dedupSet(key, id) {
  dedup[key] = { id, at: Date.now() };
  const keys = Object.keys(dedup);
  if (keys.length > DEDUP_MAX) {
    keys.sort((a, b) => (dedup[a].at || 0) - (dedup[b].at || 0));
    for (const k of keys.slice(0, keys.length - DEDUP_MAX)) delete dedup[k];
  }
  persistDedup();
}

// ---- helpers ---------------------------------------------------------------
function safeEqual(a, b) {
  const ab = Buffer.from(String(a));
  const bb = Buffer.from(String(b));
  if (ab.length !== bb.length) return false;
  return crypto.timingSafeEqual(ab, bb);
}

async function rpc(pathname, { method = 'GET', token, body } = {}) {
  const headers = { accept: 'application/json' };
  if (body !== undefined) headers['content-type'] = 'application/json';
  if (token) headers.authorization = `Bearer ${token}`;
  return fetch(`${REMOTE_URL}${pathname}`, {
    method,
    headers,
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
}

// Serialize auth so concurrent requests don't trigger multiple logins/refreshes.
let authLock = Promise.resolve();
function withAuthLock(fn) {
  const next = authLock.then(fn, fn);
  authLock = next.catch(() => {});
  return next;
}
async function login() {
  const res = await rpc('/v1/auth/local/login', {
    method: 'POST',
    body: { email: SVC_EMAIL, password: SVC_PASSWORD },
  });
  if (!res.ok) throw new Error(`login failed: HTTP ${res.status} ${await res.text()}`);
  const j = await res.json();
  tokens = { access: j.access_token, refresh: j.refresh_token };
  console.error('[ingest] service account logged in');
}
async function refresh() {
  if (!tokens.refresh) return login();
  const res = await rpc('/v1/tokens/refresh', {
    method: 'POST',
    body: { refresh_token: tokens.refresh },
  });
  if (!res.ok) {
    console.error(`[ingest] refresh failed (HTTP ${res.status}); re-logging in`);
    return login();
  }
  const j = await res.json();
  tokens = { access: j.access_token, refresh: j.refresh_token };
}

// authenticated call with one transparent retry on 401
async function authed(pathname, opts = {}) {
  if (!tokens.access) await withAuthLock(() => (tokens.access ? Promise.resolve() : login()));
  let res = await rpc(pathname, { ...opts, token: tokens.access });
  if (res.status === 401) {
    await withAuthLock(refresh);
    res = await rpc(pathname, { ...opts, token: tokens.access });
  }
  return res;
}

async function resolveStatusId() {
  if (statusId && (statusAt === Infinity || Date.now() - statusAt < STATUS_TTL_MS)) return statusId;
  const res = await authed(`/v1/project_statuses?project_id=${encodeURIComponent(PROJECT_ID)}`);
  if (!res.ok) throw new Error(`list statuses failed: HTTP ${res.status} ${await res.text()}`);
  const j = await res.json();
  const list = j.project_statuses || j.data || [];
  if (!list.length) throw new Error('project has no statuses');
  const byName = list.find((s) => (s.name || '').toLowerCase().includes(STATUS_NAME_HINT));
  const chosen = byName || [...list].sort((a, b) => (a.sort_order ?? 0) - (b.sort_order ?? 0))[0];
  statusId = chosen.id;
  statusAt = Date.now();
  console.error(`[ingest] default status -> "${chosen.name}" (${statusId})`);
  return statusId;
}

// Resolve an assignee email -> user_id via the org member list (cached ~5 min).
let memberCache = { at: 0, byEmail: new Map() };
async function getMemberMap(force = false) {
  if (!force && Date.now() - memberCache.at < MEMBER_TTL_MS && memberCache.byEmail.size) {
    return memberCache.byEmail;
  }
  const res = await authed(`/v1/organizations/${encodeURIComponent(ORG_ID)}/members`);
  if (!res.ok) throw new Error(`list members failed: HTTP ${res.status} ${await res.text()}`);
  const j = await res.json();
  const map = new Map();
  for (const m of j.members || j.data || []) {
    if (m.email) map.set(String(m.email).toLowerCase(), m.user_id);
  }
  memberCache = { at: Date.now(), byEmail: map };
  return map;
}

// Behavior (B): never fail issue creation over assignment — report and move on.
async function tryAssign(issueId, assignee) {
  try {
    if (!ORG_ID) return { resolved: false, reason: 'INGEST_ORG_ID not configured' };
    const key = String(assignee).trim().toLowerCase();
    let userId = (await getMemberMap()).get(key);
    if (!userId) userId = (await getMemberMap(true)).get(key); // refresh once on miss (newly-added member)
    if (!userId) return { resolved: false, reason: 'no org member with that email' };
    const res = await authed('/v1/issue_assignees', {
      method: 'POST',
      body: { issue_id: issueId, user_id: userId },
    });
    if (!res.ok) return { resolved: false, reason: `assign failed: HTTP ${res.status}` };
    return { resolved: true, user_id: userId };
  } catch (e) {
    return { resolved: false, reason: e.message };
  }
}

async function createIssue(input) {
  const sid = await resolveStatusId();
  const payload = {
    project_id: PROJECT_ID,
    status_id: sid,
    title: input.title,
    description: input.description ?? null,
    sort_order: Date.now(),
    extension_metadata: { source: 'ingest', ...(input.dedup_key ? { dedup_key: input.dedup_key } : {}) },
  };
  if (input.priority) {
    const p = PRIORITIES[String(input.priority).toLowerCase()];
    if (!p) throw httpErr(400, `invalid priority "${input.priority}" (use Urgent|High|Medium|Low)`);
    payload.priority = p;
  }
  const res = await authed('/v1/issues', { method: 'POST', body: payload });
  if (!res.ok) throw httpErr(502, `create issue failed: HTTP ${res.status} ${await res.text()}`);
  const j = await res.json();
  const id = j?.data?.id || j?.id;
  const out = { id, url: PUBLIC_URL ? `${PUBLIC_URL}/projects/${PROJECT_ID}/issues/${id}` : undefined };
  if (input.assignee) out.assignee = await tryAssign(id, input.assignee);
  return out;
}

// ---- tiny http layer -------------------------------------------------------
function httpErr(status, message) {
  const e = new Error(message);
  e.status = status;
  return e;
}
function send(res, status, obj) {
  const body = JSON.stringify(obj);
  res.writeHead(status, { 'content-type': 'application/json; charset=utf-8' });
  res.end(body);
}
function readBody(req, limit = 64 * 1024) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    let size = 0;
    req.on('data', (c) => {
      size += c.length; // Buffer length = bytes
      if (size > limit) {
        req.destroy();
        reject(httpErr(413, 'payload too large'));
        return;
      }
      chunks.push(c);
    });
    req.on('end', () => resolve(Buffer.concat(chunks).toString('utf8')));
    req.on('error', reject);
  });
}
function checkApiKey(req) {
  const hdr = req.headers['x-api-key'] || (req.headers.authorization || '').replace(/^Bearer\s+/i, '');
  return Boolean(hdr) && safeEqual(hdr, API_KEY);
}

async function handleIngest(req, res) {
  if (!checkApiKey(req)) return send(res, 401, { error: 'unauthorized' });
  const raw = await readBody(req);
  let body;
  try {
    body = JSON.parse(raw || '{}');
  } catch {
    return send(res, 400, { error: 'invalid JSON' });
  }
  if (!body.title || typeof body.title !== 'string') {
    return send(res, 400, { error: 'title is required' });
  }

  const key = body.dedup_key ? String(body.dedup_key) : null;
  if (key) {
    const hit = dedupGet(key);
    if (hit) return send(res, 200, { deduped: true, id: hit.id });
    if (inFlight.has(key)) return send(res, 409, { error: 'duplicate request in progress', dedup_key: key });
    inFlight.add(key);
  }
  try {
    const created = await createIssue(body);
    if (key) dedupSet(key, created.id);
    return send(res, 201, { created: true, ...created });
  } finally {
    if (key) inFlight.delete(key);
  }
}

const server = http.createServer(async (req, res) => {
  let pathname = req.url;
  try {
    pathname = new URL(req.url, 'http://localhost').pathname;
  } catch {
    /* keep raw */
  }
  // Normalize a trailing slash so `/ingest/issues/` matches `/ingest/issues`.
  if (pathname.length > 1) pathname = pathname.replace(/\/+$/, '');
  try {
    if (req.method === 'GET' && pathname === '/health') {
      return send(res, 200, { status: 'ok' });
    }
    if (req.method === 'POST' && (pathname === '/ingest/issues' || pathname === '/issues')) {
      return await handleIngest(req, res);
    }
    return send(res, 404, { error: 'not found' });
  } catch (e) {
    const status = e.status || 500;
    if (status >= 500) console.error(`[ingest] ERROR: ${e.message}`);
    if (!res.headersSent) return send(res, status, { error: e.message });
  }
});

server.listen(PORT, () => {
  console.error(`[ingest] listening on :${PORT} -> ${REMOTE_URL} (project ${PROJECT_ID})`);
});

// Graceful shutdown so in-flight requests finish on redeploy/restart.
function shutdown(sig) {
  console.error(`[ingest] ${sig} received — shutting down`);
  server.close(() => process.exit(0));
  setTimeout(() => process.exit(0), 10000).unref();
}
process.on('SIGTERM', () => shutdown('SIGTERM'));
process.on('SIGINT', () => shutdown('SIGINT'));
