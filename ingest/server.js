'use strict';
// Vibe Kanban issue-ingestion sidecar.
// POST /ingest/issues  -> creates an issue on the central server via /v1/issues,
// authenticating as a service account (self-host local auth). API-key guarded.
//
// Required env: INGEST_API_KEY, INGEST_SVC_EMAIL, INGEST_SVC_PASSWORD, INGEST_PROJECT_ID
// Optional env: INGEST_PORT(8090), REMOTE_URL(http://remote:8081), INGEST_ORG_ID,
//               INGEST_STATUS_ID, INGEST_STATUS_NAME(todo), INGEST_DEDUP_FILE(/data/dedup.json),
//               INGEST_DEDUP_TTL_DAYS(30), INGEST_PUBLIC_URL,
//               INGEST_REQUIREMENTS_FILE(/data/requirements-drafts.json),
//               INGEST_MAX_BODY_KB(2048), INGEST_MAX_CHILD_TASKS(12),
//               R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY, R2_REVIEW_ENDPOINT,
//               R2_REVIEW_BUCKET, R2_PRESIGN_EXPIRY_SECS(300),
//               INGEST_UPLOAD_MAX_MB(20), R2_REGION(auto)

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
const REQUIREMENTS_FILE = process.env.INGEST_REQUIREMENTS_FILE || '/data/requirements-drafts.json';
const BODY_LIMIT_BYTES = (parseInt(process.env.INGEST_MAX_BODY_KB || '2048', 10) || 2048) * 1024;
const MAX_CHILD_TASKS = parseInt(process.env.INGEST_MAX_CHILD_TASKS || '12', 10) || 12;
const R2_ACCESS_KEY_ID = process.env.R2_ACCESS_KEY_ID || '';
const R2_SECRET_ACCESS_KEY = process.env.R2_SECRET_ACCESS_KEY || '';
const R2_ENDPOINT = (process.env.R2_REVIEW_ENDPOINT || '').replace(/\/+$/, '');
const R2_BUCKET = process.env.R2_REVIEW_BUCKET || '';
const R2_REGION = process.env.R2_REGION || 'auto';
const R2_PRESIGN_EXPIRY_SECS = parseInt(process.env.R2_PRESIGN_EXPIRY_SECS || '300', 10) || 300;
const UPLOAD_LIMIT_BYTES = (parseInt(process.env.INGEST_UPLOAD_MAX_MB || '20', 10) || 20) * 1024 * 1024;
// Server expects lowercase enum variants: urgent | high | medium | low.
const PRIORITIES = { urgent: 'urgent', high: 'high', medium: 'medium', low: 'low' };
const SOURCE_TYPES = new Set(['pdf', 'docx', 'markdown']);

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
let requirementState = {
  drafts: {},
  source_index: {},
};

function persistDedup() {
  try {
    fs.mkdirSync(path.dirname(DEDUP_FILE), { recursive: true });
    fs.writeFileSync(DEDUP_FILE, JSON.stringify(dedup));
  } catch (e) {
    console.error(`[ingest] WARN: could not persist dedup store: ${e.message}`);
  }
}

try {
  requirementState = JSON.parse(fs.readFileSync(REQUIREMENTS_FILE, 'utf8')) || requirementState;
} catch {
  requirementState = { drafts: {}, source_index: {} };
}

function persistRequirements() {
  try {
    fs.mkdirSync(path.dirname(REQUIREMENTS_FILE), { recursive: true });
    fs.writeFileSync(REQUIREMENTS_FILE, JSON.stringify(requirementState));
  } catch (e) {
    console.error(`[ingest] WARN: could not persist requirements store: ${e.message}`);
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

function hasR2PresignConfig() {
  return Boolean(R2_ACCESS_KEY_ID && R2_SECRET_ACCESS_KEY && R2_ENDPOINT && R2_BUCKET);
}

function awsEncode(value) {
  return encodeURIComponent(String(value)).replace(/[!'()*]/g, (ch) =>
    `%${ch.charCodeAt(0).toString(16).toUpperCase()}`
  );
}

function formatAmzDate(date) {
  const yyyy = date.getUTCFullYear();
  const mm = String(date.getUTCMonth() + 1).padStart(2, '0');
  const dd = String(date.getUTCDate()).padStart(2, '0');
  const hh = String(date.getUTCHours()).padStart(2, '0');
  const min = String(date.getUTCMinutes()).padStart(2, '0');
  const sec = String(date.getUTCSeconds()).padStart(2, '0');
  return `${yyyy}${mm}${dd}T${hh}${min}${sec}Z`;
}

function normalizeObjectKey(key) {
  const clean = String(key || '').trim().replace(/^\/+/, '');
  if (!clean) throw httpErr(400, 'source.object_keys contains an empty key');
  return clean;
}

function signHmac(key, msg, encoding = undefined) {
  const h = crypto.createHmac('sha256', key).update(msg);
  return encoding ? h.digest(encoding) : h.digest();
}

function presignR2Object(method, objectKey, expiresSecs = R2_PRESIGN_EXPIRY_SECS) {
  if (!hasR2PresignConfig()) {
    throw httpErr(503, 'R2 signing not configured on ingest service');
  }
  const upperMethod = String(method || 'GET').toUpperCase();
  if (!['GET', 'PUT'].includes(upperMethod)) {
    throw httpErr(400, `unsupported presign method: ${upperMethod}`);
  }
  const ttl = Math.max(60, Math.min(3600, parseInt(expiresSecs, 10) || R2_PRESIGN_EXPIRY_SECS));
  const endpoint = new URL(R2_ENDPOINT);
  const host = endpoint.host;
  const now = new Date();
  const amzDate = formatAmzDate(now);
  const dateStamp = amzDate.slice(0, 8);
  const credentialScope = `${dateStamp}/${R2_REGION}/s3/aws4_request`;
  const encodedKey = normalizeObjectKey(objectKey)
    .split('/')
    .map(awsEncode)
    .join('/');
  const basePath = endpoint.pathname && endpoint.pathname !== '/' ? endpoint.pathname.replace(/\/+$/, '') : '';
  const canonicalUri = `${basePath}/${awsEncode(R2_BUCKET)}/${encodedKey}`;
  const query = {
    'X-Amz-Algorithm': 'AWS4-HMAC-SHA256',
    'X-Amz-Credential': `${R2_ACCESS_KEY_ID}/${credentialScope}`,
    'X-Amz-Date': amzDate,
    'X-Amz-Expires': String(ttl),
    'X-Amz-SignedHeaders': 'host',
  };
  const canonicalQuery = Object.keys(query)
    .sort()
    .map((k) => `${awsEncode(k)}=${awsEncode(query[k])}`)
    .join('&');
  const canonicalHeaders = `host:${host}\n`;
  const canonicalRequest = [
    upperMethod,
    canonicalUri,
    canonicalQuery,
    canonicalHeaders,
    'host',
    'UNSIGNED-PAYLOAD',
  ].join('\n');
  const stringToSign = [
    'AWS4-HMAC-SHA256',
    amzDate,
    credentialScope,
    crypto.createHash('sha256').update(canonicalRequest).digest('hex'),
  ].join('\n');
  const kDate = signHmac(`AWS4${R2_SECRET_ACCESS_KEY}`, dateStamp);
  const kRegion = signHmac(kDate, R2_REGION);
  const kService = signHmac(kRegion, 's3');
  const kSigning = signHmac(kService, 'aws4_request');
  const signature = signHmac(kSigning, stringToSign, 'hex');
  const signedQuery = `${canonicalQuery}&X-Amz-Signature=${signature}`;
  return `${endpoint.origin}${canonicalUri}?${signedQuery}`;
}

function presignR2ObjectGet(objectKey, expiresSecs = R2_PRESIGN_EXPIRY_SECS) {
  return presignR2Object('GET', objectKey, expiresSecs);
}

function presignR2ObjectPut(objectKey, expiresSecs = R2_PRESIGN_EXPIRY_SECS) {
  return presignR2Object('PUT', objectKey, expiresSecs);
}

function buildR2DurableUri(objectKey) {
  return `r2://${R2_BUCKET}/${normalizeObjectKey(objectKey)}`;
}

function sanitizeFileName(fileName) {
  const cleaned = String(fileName || '')
    .trim()
    .replace(/^.*[\\/]/, '')
    .replace(/[^a-zA-Z0-9._-]+/g, '-')
    .replace(/-+/g, '-')
    .replace(/^-+|-+$/g, '');
  return cleaned || 'source.bin';
}

function defaultObjectKey(fileName, prefix = 'requirements/uploads') {
  const d = new Date();
  const y = d.getUTCFullYear();
  const m = String(d.getUTCMonth() + 1).padStart(2, '0');
  const id = crypto.randomUUID();
  const safe = sanitizeFileName(fileName);
  return `${prefix}/${y}/${m}/${id}-${safe}`;
}

async function uploadR2Object(objectKey, payloadBuffer, contentType = 'application/octet-stream') {
  if (!hasR2PresignConfig()) {
    throw httpErr(503, 'R2 signing not configured on ingest service');
  }
  if (!Buffer.isBuffer(payloadBuffer) || !payloadBuffer.length) {
    throw httpErr(400, 'upload payload is empty');
  }
  const cleanKey = normalizeObjectKey(objectKey);
  const putUrl = presignR2ObjectPut(cleanKey);
  const res = await fetch(putUrl, {
    method: 'PUT',
    headers: {
      'content-type': contentType || 'application/octet-stream',
    },
    body: payloadBuffer,
  });
  if (!res.ok) {
    throw new Error(`R2 upload failed: HTTP ${res.status} ${await res.text()}`);
  }
  return {
    object_key: cleanKey,
    durable_uri: buildR2DurableUri(cleanKey),
    signed_get_url: presignR2ObjectGet(cleanKey),
    expires_in_secs: Math.max(60, Math.min(3600, R2_PRESIGN_EXPIRY_SECS)),
    size_bytes: payloadBuffer.length,
    content_type: contentType || 'application/octet-stream',
  };
}

function parseMultipartForm(buffer, boundary) {
  const out = { fields: {}, file: null };
  const marker = `--${boundary}`;
  const body = buffer.toString('latin1');
  const chunks = body.split(marker);
  for (const chunk of chunks) {
    const trimmed = chunk.trim();
    if (!trimmed || trimmed === '--') continue;
    const normalized = chunk.startsWith('\r\n') ? chunk.slice(2) : chunk;
    const sep = normalized.indexOf('\r\n\r\n');
    if (sep < 0) continue;
    const headerText = normalized.slice(0, sep);
    let contentText = normalized.slice(sep + 4);
    if (contentText.endsWith('\r\n')) contentText = contentText.slice(0, -2);
    const headers = {};
    for (const line of headerText.split('\r\n')) {
      const idx = line.indexOf(':');
      if (idx < 0) continue;
      const key = line.slice(0, idx).trim().toLowerCase();
      const value = line.slice(idx + 1).trim();
      headers[key] = value;
    }
    const disposition = headers['content-disposition'] || '';
    const nameMatch = disposition.match(/name="([^"]+)"/i);
    if (!nameMatch) continue;
    const fieldName = nameMatch[1];
    const fileMatch = disposition.match(/filename="([^"]*)"/i);
    if (fileMatch) {
      out.file = {
        field_name: fieldName,
        filename: sanitizeFileName(fileMatch[1] || ''),
        content_type: headers['content-type'] || 'application/octet-stream',
        buffer: Buffer.from(contentText, 'latin1'),
      };
      continue;
    }
    out.fields[fieldName] = Buffer.from(contentText, 'latin1').toString('utf8').trim();
  }
  return out;
}

function buildSignedSourceLinks(sourceObjectKeys, ttlSecs = R2_PRESIGN_EXPIRY_SECS) {
  if (!Array.isArray(sourceObjectKeys) || !sourceObjectKeys.length) return [];
  if (!hasR2PresignConfig()) return [];
  return sourceObjectKeys.map((key) => {
    const clean = normalizeObjectKey(key);
    return {
      key: clean,
      url: presignR2ObjectGet(clean, ttlSecs),
      expires_in_secs: Math.max(60, Math.min(3600, parseInt(ttlSecs, 10) || R2_PRESIGN_EXPIRY_SECS)),
    };
  });
}

function withSignedLinks(draft, ttlSecs = R2_PRESIGN_EXPIRY_SECS) {
  return {
    ...draft,
    signed_source_links: buildSignedSourceLinks(draft.source_object_keys || [], ttlSecs),
  };
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

function toMarkdownList(lines) {
  if (!Array.isArray(lines) || !lines.length) return '';
  return lines.map((line) => `- ${String(line).trim()}`).join('\n');
}

function normalizePriority(priority) {
  if (!priority) return null;
  return PRIORITIES[String(priority).trim().toLowerCase()] || null;
}

function buildEpicDescription({ summary, acceptanceCriteria, sourceLinks, revision }) {
  const ac = toMarkdownList(acceptanceCriteria);
  const src = toMarkdownList(sourceLinks);
  const chunks = [
    '## Summary',
    summary || 'No summary provided.',
  ];
  if (ac) {
    chunks.push('', '## Acceptance Criteria', ac);
  }
  if (src) {
    chunks.push('', '## Source Links', src);
  }
  chunks.push('', `> Requirement revision: v${revision}`);
  return chunks.join('\n');
}

function buildTaskDescription(task, sourceLinks = []) {
  const objective = String(task.objective || '').trim();
  const ac = toMarkdownList(task.acceptance_criteria || []);
  const src = toMarkdownList(sourceLinks);
  const sections = [];
  if (objective) {
    sections.push('## Objective', objective);
  }
  if (ac) {
    sections.push('', '## Acceptance Criteria', ac);
  }
  if (src) {
    sections.push('', '## Source Links', src);
  }
  return sections.join('\n') || null;
}

function capChildTasks(tasks, epicTitle) {
  if (!Array.isArray(tasks) || !tasks.length) return [];
  if (tasks.length <= MAX_CHILD_TASKS) return tasks;
  if (MAX_CHILD_TASKS <= 1) {
    return [{
      title: `[Follow-up] Remaining scope for ${epicTitle}`,
      objective: `Agent found ${tasks.length} tasks; child task cap is ${MAX_CHILD_TASKS}. Review and split manually.`,
      acceptance_criteria: ['Review deferred scope and create additional tasks if needed.'],
      priority: 'medium',
    }];
  }
  const selected = tasks.slice(0, MAX_CHILD_TASKS - 1);
  const remaining = tasks.length - (MAX_CHILD_TASKS - 1);
  selected.push({
    title: `[Follow-up] Remaining scope for ${epicTitle}`,
    objective: `Agent found ${remaining} additional tasks beyond cap (${MAX_CHILD_TASKS}). Review and split manually.`,
    acceptance_criteria: ['Review deferred scope and create additional tasks if needed.'],
    priority: 'medium',
  });
  return selected;
}

function deriveTasksFromAc(epicTitle, acceptanceCriteria) {
  if (!Array.isArray(acceptanceCriteria) || !acceptanceCriteria.length) return [];
  return acceptanceCriteria.map((item, idx) => ({
    title: `${epicTitle} - AC ${idx + 1}`,
    objective: String(item || '').trim(),
    acceptance_criteria: [String(item || '').trim()],
  }));
}

function validateRequirementDraftPayload(body) {
  if (!body || typeof body !== 'object') return 'body must be an object';
  if (!body.source || typeof body.source !== 'object') return 'source is required';
  if (!SOURCE_TYPES.has(String(body.source.type || '').toLowerCase())) {
    return 'source.type must be one of: pdf|docx|markdown';
  }
  if (!body.source.fingerprint || typeof body.source.fingerprint !== 'string') {
    return 'source.fingerprint is required';
  }
  if (body.source.object_keys !== undefined) {
    if (!Array.isArray(body.source.object_keys)) {
      return 'source.object_keys must be an array of strings when provided';
    }
    for (const key of body.source.object_keys) {
      if (typeof key !== 'string' || !String(key).trim()) {
        return 'source.object_keys must contain non-empty strings';
      }
    }
    if (body.source.object_keys.length && !hasR2PresignConfig()) {
      return 'R2 signing is not configured; set R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY, R2_REVIEW_ENDPOINT, R2_REVIEW_BUCKET';
    }
  }
  if (!body.epic || typeof body.epic !== 'object') return 'epic is required';
  if (!body.epic.title || typeof body.epic.title !== 'string') {
    return 'epic.title is required';
  }
  if (!body.epic.summary || typeof body.epic.summary !== 'string') {
    return 'epic.summary is required';
  }
  if (!Array.isArray(body.epic.acceptance_criteria)) {
    return 'epic.acceptance_criteria must be an array of strings';
  }
  if (body.child_tasks !== undefined && !Array.isArray(body.child_tasks)) {
    return 'child_tasks must be an array when provided';
  }
  return null;
}

async function upsertEpicForDraft(draft) {
  const sourceState = requirementState.source_index[draft.source_fingerprint];
  const description = buildEpicDescription({
    summary: draft.epic.summary,
    acceptanceCriteria: draft.epic.acceptance_criteria,
    sourceLinks: draft.source_links,
    revision: draft.revision_number,
  });
  const extensionMetadata = {
    source: 'ingest-agent-centric',
    source_type: draft.source_type,
    source_fingerprint: draft.source_fingerprint,
    revision_number: draft.revision_number,
    generated_by: draft.generated_by || null,
    metadata: draft.metadata || {},
  };

  if (sourceState?.epic_issue_id) {
    const res = await authed('/v1/issues/bulk', {
      method: 'POST',
      body: {
        updates: [
          {
            id: sourceState.epic_issue_id,
            title: draft.epic.title,
            description,
            extension_metadata: extensionMetadata,
          },
        ],
      },
    });
    if (!res.ok) throw new Error(`update epic failed: HTTP ${res.status} ${await res.text()}`);
    const body = await res.json();
    return body?.data?.[0]?.id || sourceState.epic_issue_id;
  }

  const created = await createIssue({
    title: draft.epic.title,
    description,
    dedup_key: null,
  });
  const epicId = created.id;
  const patchRes = await authed('/v1/issues/bulk', {
    method: 'POST',
    body: {
      updates: [
        {
          id: epicId,
          extension_metadata: extensionMetadata,
        },
      ],
    },
  });
  if (!patchRes.ok) {
    throw new Error(`set epic metadata failed: HTTP ${patchRes.status} ${await patchRes.text()}`);
  }
  return epicId;
}

async function createChildTask(epicId, task, sourceLinks) {
  const payload = {
    title: task.title,
    description: buildTaskDescription(task, sourceLinks),
    priority: task.priority ? String(task.priority) : null,
  };
  const created = await createIssue({
    ...payload,
    dedup_key: null,
  });
  const childId = created.id;
  const updateRes = await authed('/v1/issues/bulk', {
    method: 'POST',
    body: {
      updates: [
        {
          id: childId,
          parent_issue_id: epicId,
        },
      ],
    },
  });
  if (!updateRes.ok) {
    throw new Error(`set parent issue failed: HTTP ${updateRes.status} ${await updateRes.text()}`);
  }
  const result = { id: childId };
  if (task.assignee) {
    result.assignee = await tryAssign(childId, task.assignee);
  }
  return result;
}

async function publishRequirementDraft(draft) {
  const epicIssueId = await upsertEpicForDraft(draft);
  const childIssueIds = [];
  for (const task of draft.child_tasks) {
    const created = await createChildTask(epicIssueId, task, draft.source_links);
    childIssueIds.push(created);
  }
  requirementState.source_index[draft.source_fingerprint] = {
    epic_issue_id: epicIssueId,
    last_revision_number: draft.revision_number,
    last_published_draft_id: draft.id,
    last_published_at: new Date().toISOString(),
  };
  persistRequirements();
  return { epic_issue_id: epicIssueId, child_issues: childIssueIds };
}

async function handleCreateRequirementDraft(req, res) {
  if (!checkApiKey(req)) return send(res, 401, { error: 'unauthorized' });
  const raw = await readBody(req, BODY_LIMIT_BYTES);
  let body;
  try {
    body = JSON.parse(raw || '{}');
  } catch {
    return send(res, 400, { error: 'invalid JSON' });
  }
  const validationError = validateRequirementDraftPayload(body);
  if (validationError) return send(res, 400, { error: validationError });

  const sourceType = String(body.source.type).toLowerCase();
  const sourceFingerprint = String(body.source.fingerprint).trim();
  const sourceLinks = Array.isArray(body.source.links) ? body.source.links.map(String) : [];
  const sourceObjectKeys = Array.isArray(body.source.object_keys)
    ? body.source.object_keys.map((key) => normalizeObjectKey(key))
    : [];
  const durableSourceLinks = [
    ...sourceLinks,
    ...sourceObjectKeys.map((key) => buildR2DurableUri(key)),
  ];
  const existing = requirementState.source_index[sourceFingerprint];
  const revisionNumber = existing ? (existing.last_revision_number || 0) + 1 : 1;
  const changeType = existing ? 'revision' : 'new';
  const suppliedTasks = Array.isArray(body.child_tasks) ? body.child_tasks : [];
  const fallbackTasks = suppliedTasks.length
    ? suppliedTasks
    : deriveTasksFromAc(body.epic.title, body.epic.acceptance_criteria);
  const childTasks = capChildTasks(fallbackTasks, body.epic.title);
  const now = new Date().toISOString();
  const id = crypto.randomUUID();
  const draft = {
    id,
    status: 'draft',
    requires_approval: true,
    change_type: changeType,
    created_at: now,
    source_type: sourceType,
    source_fingerprint: sourceFingerprint,
    source_links: [...new Set(durableSourceLinks)],
    source_object_keys: sourceObjectKeys,
    revision_number: revisionNumber,
    generated_by: body.generated_by || 'agent',
    metadata: body.metadata && typeof body.metadata === 'object' ? body.metadata : {},
    epic: {
      title: String(body.epic.title).trim(),
      summary: String(body.epic.summary).trim(),
      acceptance_criteria: body.epic.acceptance_criteria.map((item) => String(item)),
    },
    child_tasks: childTasks.map((task, idx) => {
      const priority = normalizePriority(task.priority);
      return {
        id: `${id}-task-${idx + 1}`,
        title: String(task.title || '').trim(),
        objective: String(task.objective || '').trim(),
        acceptance_criteria: Array.isArray(task.acceptance_criteria)
          ? task.acceptance_criteria.map((item) => String(item))
          : [],
        priority,
        assignee: task.assignee ? String(task.assignee).trim() : null,
      };
    }).filter((task) => task.title),
  };

  requirementState.drafts[id] = draft;
  persistRequirements();
  const signedSourceLinks = buildSignedSourceLinks(sourceObjectKeys);
  return send(res, 201, {
    created: true,
    draft_id: id,
    status: draft.status,
    requires_approval: true,
    change_type: draft.change_type,
    revision_number: draft.revision_number,
    child_task_count: draft.child_tasks.length,
    signed_source_links: signedSourceLinks,
  });
}

async function handleUploadRequirementSourcePut(req, res, rawObjectPath) {
  if (!checkApiKey(req)) return send(res, 401, { error: 'unauthorized' });
  if (!hasR2PresignConfig()) {
    return send(res, 503, { error: 'R2 signing not configured on ingest service' });
  }
  const url = new URL(req.url, 'http://localhost');
  const objectKey = rawObjectPath
    ? decodeURIComponent(rawObjectPath)
    : (url.searchParams.get('object_key') || '').trim();
  if (!objectKey) {
    return send(res, 400, { error: 'object key is required in path or object_key query' });
  }
  const body = await readBodyBuffer(req, UPLOAD_LIMIT_BYTES);
  if (!body.length) return send(res, 400, { error: 'empty upload body' });
  const contentType = req.headers['content-type'] || 'application/octet-stream';
  const uploaded = await uploadR2Object(objectKey, body, contentType);
  return send(res, 201, { uploaded: true, ...uploaded });
}

async function handleUploadRequirementSourceMultipart(req, res) {
  if (!checkApiKey(req)) return send(res, 401, { error: 'unauthorized' });
  if (!hasR2PresignConfig()) {
    return send(res, 503, { error: 'R2 signing not configured on ingest service' });
  }
  const contentType = req.headers['content-type'] || '';
  const m = String(contentType).match(/boundary=([^;]+)/i);
  if (!m) return send(res, 400, { error: 'multipart boundary is missing' });
  const boundary = m[1].replace(/^"|"$/g, '');
  const raw = await readBodyBuffer(req, UPLOAD_LIMIT_BYTES);
  const parsed = parseMultipartForm(raw, boundary);
  if (!parsed.file || !parsed.file.buffer || !parsed.file.buffer.length) {
    return send(res, 400, { error: 'multipart file field is required' });
  }
  const explicitKey = parsed.fields.object_key ? String(parsed.fields.object_key).trim() : '';
  const prefix = parsed.fields.prefix ? String(parsed.fields.prefix).trim() : 'requirements/uploads';
  const objectKey = explicitKey || defaultObjectKey(parsed.file.filename, prefix);
  const uploaded = await uploadR2Object(objectKey, parsed.file.buffer, parsed.file.content_type);
  return send(res, 201, {
    uploaded: true,
    filename: parsed.file.filename,
    ...uploaded,
  });
}

function listRequirementDrafts(status) {
  const items = Object.values(requirementState.drafts);
  return items
    .filter((item) => (!status ? true : item.status === status))
    .sort((a, b) => String(b.created_at).localeCompare(String(a.created_at)));
}

async function handleListRequirementDrafts(req, res) {
  if (!checkApiKey(req)) return send(res, 401, { error: 'unauthorized' });
  const url = new URL(req.url, 'http://localhost');
  const status = url.searchParams.get('status');
  const ttl = parseInt(url.searchParams.get('ttl_secs') || String(R2_PRESIGN_EXPIRY_SECS), 10);
  return send(res, 200, {
    drafts: listRequirementDrafts(status).map((draft) => withSignedLinks(draft, ttl)),
  });
}

async function handleGetRequirementDraft(req, res, draftId) {
  if (!checkApiKey(req)) return send(res, 401, { error: 'unauthorized' });
  const draft = requirementState.drafts[draftId];
  if (!draft) return send(res, 404, { error: 'draft not found' });
  const url = new URL(req.url, 'http://localhost');
  const ttl = parseInt(url.searchParams.get('ttl_secs') || String(R2_PRESIGN_EXPIRY_SECS), 10);
  return send(res, 200, withSignedLinks(draft, ttl));
}

async function handleGetRequirementDraftSignedLinks(req, res, draftId) {
  if (!checkApiKey(req)) return send(res, 401, { error: 'unauthorized' });
  const draft = requirementState.drafts[draftId];
  if (!draft) return send(res, 404, { error: 'draft not found' });
  if (!draft.source_object_keys || !draft.source_object_keys.length) {
    return send(res, 200, { draft_id: draftId, signed_source_links: [] });
  }
  if (!hasR2PresignConfig()) {
    return send(res, 503, { error: 'R2 signing not configured on ingest service' });
  }
  const url = new URL(req.url, 'http://localhost');
  const ttl = parseInt(url.searchParams.get('ttl_secs') || String(R2_PRESIGN_EXPIRY_SECS), 10);
  return send(res, 200, {
    draft_id: draftId,
    signed_source_links: buildSignedSourceLinks(draft.source_object_keys, ttl),
  });
}

async function handleApproveRequirementDraft(req, res, draftId) {
  if (!checkApiKey(req)) return send(res, 401, { error: 'unauthorized' });
  const draft = requirementState.drafts[draftId];
  if (!draft) return send(res, 404, { error: 'draft not found' });
  if (draft.status !== 'draft') {
    return send(res, 409, { error: `draft is already ${draft.status}` });
  }
  let approver = 'human-reviewer';
  try {
    const raw = await readBody(req, BODY_LIMIT_BYTES);
    if (raw) {
      const body = JSON.parse(raw);
      if (body.approved_by) approver = String(body.approved_by);
    }
  } catch {
    // Empty/invalid body should not block approval.
  }

  const published = await publishRequirementDraft(draft);
  draft.status = 'published';
  draft.approved_at = new Date().toISOString();
  draft.approved_by = approver;
  draft.published = published;
  persistRequirements();
  const signedSourceLinks = buildSignedSourceLinks(draft.source_object_keys || []);
  return send(res, 200, {
    approved: true,
    draft_id: draft.id,
    ...published,
    signed_source_links: signedSourceLinks,
  });
}

async function handleRejectRequirementDraft(req, res, draftId) {
  if (!checkApiKey(req)) return send(res, 401, { error: 'unauthorized' });
  const draft = requirementState.drafts[draftId];
  if (!draft) return send(res, 404, { error: 'draft not found' });
  if (draft.status !== 'draft') {
    return send(res, 409, { error: `draft is already ${draft.status}` });
  }
  let reason = null;
  try {
    const raw = await readBody(req, BODY_LIMIT_BYTES);
    if (raw) {
      const body = JSON.parse(raw);
      if (body.reason) reason = String(body.reason);
    }
  } catch {
    // Ignore malformed optional reject payload.
  }
  draft.status = 'rejected';
  draft.rejected_at = new Date().toISOString();
  draft.rejection_reason = reason;
  persistRequirements();
  return send(res, 200, { rejected: true, draft_id: draft.id });
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
function readBodyBuffer(req, limit = BODY_LIMIT_BYTES) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    let size = 0;
    req.on('data', (c) => {
      size += c.length;
      if (size > limit) {
        req.destroy();
        reject(httpErr(413, 'payload too large'));
        return;
      }
      chunks.push(c);
    });
    req.on('end', () => resolve(Buffer.concat(chunks)));
    req.on('error', reject);
  });
}
function readBody(req, limit = BODY_LIMIT_BYTES) {
  return readBodyBuffer(req, limit).then((b) => b.toString('utf8'));
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
    if (req.method === 'POST' && pathname === '/ingest/requirements/drafts') {
      return await handleCreateRequirementDraft(req, res);
    }
    if (req.method === 'POST' && pathname === '/ingest/requirements/sources/upload') {
      return await handleUploadRequirementSourceMultipart(req, res);
    }
    const sourceUploadPath = pathname.match(/^\/ingest\/requirements\/sources\/(.+)$/);
    if (sourceUploadPath && req.method === 'PUT') {
      return await handleUploadRequirementSourcePut(req, res, sourceUploadPath[1]);
    }
    if (req.method === 'GET' && pathname === '/ingest/requirements/drafts') {
      return await handleListRequirementDrafts(req, res);
    }
    const draftPath = pathname.match(/^\/ingest\/requirements\/drafts\/([0-9a-f-]+)(?:\/(approve|reject))?$/);
    if (draftPath) {
      const [, draftId, action] = draftPath;
      if (req.method === 'GET' && !action) {
        return await handleGetRequirementDraft(req, res, draftId);
      }
      if (req.method === 'POST' && action === 'approve') {
        return await handleApproveRequirementDraft(req, res, draftId);
      }
      if (req.method === 'POST' && action === 'reject') {
        return await handleRejectRequirementDraft(req, res, draftId);
      }
    }
    const draftSignedPath = pathname.match(/^\/ingest\/requirements\/drafts\/([0-9a-f-]+)\/signed-source-links$/);
    if (draftSignedPath && req.method === 'GET') {
      const [, draftId] = draftSignedPath;
      return await handleGetRequirementDraftSignedLinks(req, res, draftId);
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
