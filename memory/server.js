'use strict';

const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const readline = require('node:readline');
const { randomUUID } = require('node:crypto');
const { Client } = require('pg');

const PORT = parseInt(process.env.MEMORY_PORT || '8091', 10);
const REMOTE_URL = (process.env.REMOTE_URL || 'http://remote:8081').replace(/\/+$/, '');
const API_KEY = process.env.MEMORY_API_KEY || '';
const DB_URL = process.env.MEMORY_DATABASE_URL || '';
const EMBED_PROVIDER = (process.env.EMBED_PROVIDER || 'ollama').toLowerCase();
const EMBED_BASE_URL = (process.env.EMBED_BASE_URL || 'http://embedder:11434').replace(/\/+$/, '');
const EMBED_MODEL = process.env.EMBED_MODEL || 'nomic-embed-text';
const EMBED_DIMENSIONS = parseInt(process.env.EMBED_DIMENSIONS || '768', 10);
const EMBED_API_KEY = process.env.EMBED_API_KEY || '';
const EMBED_BATCH = parseInt(process.env.EMBED_BATCH || '32', 10);
const DOC_ROOT = process.env.MEMORY_DOC_ROOT || '/workspace';
const DOC_INCLUDE = (process.env.MEMORY_DOC_INCLUDE ||
  'HANDOFF.md,GO_LIVE.md,SYSTEM_DESIGN.md,EXECUTION_PLAN.md,DEPLOYMENT_README.md,DEVELOPER_ONBOARDING.md,REQUIREMENTS_AND_PLAN.md')
  .split(',')
  .map((x) => x.trim())
  .filter(Boolean);
const MEMORY_DEFAULT_ORG_ID = process.env.MEMORY_DEFAULT_ORG_ID || '';

const SVC_EMAIL = process.env.MEMORY_SVC_EMAIL || process.env.INGEST_SVC_EMAIL || '';
const SVC_PASSWORD = process.env.MEMORY_SVC_PASSWORD || process.env.INGEST_SVC_PASSWORD || '';

// Org isolation: a single shared API key may only act on these org(s). Defaults
// to the configured default org. Empty list = unrestricted (single-tenant dev).
const ALLOWED_ORG_IDS = (process.env.MEMORY_ALLOWED_ORG_IDS || MEMORY_DEFAULT_ORG_ID)
  .split(',')
  .map((x) => x.trim())
  .filter(Boolean);
// Retention (§7): noisy types soft-deleted after N days; soft-deleted purged after M.
const RETENTION_NOISY_DAYS = parseInt(process.env.MEMORY_RETENTION_NOISY_DAYS || '90', 10);
const RETENTION_PURGE_DAYS = parseInt(process.env.MEMORY_RETENTION_PURGE_DAYS || '30', 10);
const RETENTION_NOISY_TYPES = ['dispatch', 'issue_note', 'note'];
const EMBED_MAX_RETRIES = parseInt(process.env.EMBED_MAX_RETRIES || '4', 10);
let ollamaModelReady = false;

if (!DB_URL) {
  console.error('[memory] FATAL: MEMORY_DATABASE_URL is required');
  process.exit(1);
}

if (!SVC_EMAIL || !SVC_PASSWORD) {
  console.error('[memory] FATAL: MEMORY_SVC_EMAIL and MEMORY_SVC_PASSWORD are required');
  process.exit(1);
}

const db = new Client({ connectionString: DB_URL });
const SECRET_PATTERNS = [
  /AKIA[0-9A-Z]{16}/g, // AWS access key
  /(?:xoxb|xoxp|xoxa|xoxr)-[0-9A-Za-z-]{10,}/g, // Slack tokens
  /ghp_[0-9A-Za-z]{20,}/g, // GitHub PAT
  /sk-[A-Za-z0-9]{20,}/g, // OpenAI-like key
  /-----BEGIN [A-Z ]+PRIVATE KEY-----[\s\S]+?-----END [A-Z ]+PRIVATE KEY-----/g,
];

let tokens = { access: null, refresh: null };
let authLock = Promise.resolve();

function log(msg) {
  console.error(`[memory] ${msg}`);
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function redactSecrets(text) {
  let out = String(text || '');
  for (const re of SECRET_PATTERNS) {
    out = out.replace(re, '[REDACTED_SECRET]');
  }
  // High-entropy backstop: redact long mixed-case alphanumeric tokens (typical of
  // random secrets) but spare lowercase-hex git SHAs and UPPER_SNAKE constants to
  // avoid shredding legitimate content/citations.
  out = out.replace(/[A-Za-z0-9_\-+/=]{32,}/g, (token) => {
    const hasNum = /[0-9]/.test(token);
    const hasLower = /[a-z]/.test(token);
    const hasUpper = /[A-Z]/.test(token);
    const looksBase64 = /[+/=]/.test(token) && token.length >= 40;
    const mixedCaseSecret = hasNum && hasLower && hasUpper;
    return mixedCaseSecret || looksBase64 ? '[REDACTED_TOKEN]' : token;
  });
  return out;
}

function inferSourceType(fileName) {
  const v = fileName.toLowerCase();
  if (v.includes('runbook') || v.includes('go_live') || v.includes('handoff')) return 'runbook';
  if (v.includes('decision') || v.includes('architecture') || v.includes('design')) return 'decision';
  return 'note';
}

function chunkText(content, maxLen = 1400) {
  const blocks = String(content || '')
    .split(/\n{2,}/)
    .map((x) => x.trim())
    .filter(Boolean);
  const chunks = [];
  let cur = '';
  for (const block of blocks) {
    if (cur.length + block.length + 2 > maxLen) {
      if (cur) chunks.push(cur);
      cur = block;
    } else {
      cur = cur ? `${cur}\n\n${block}` : block;
    }
  }
  if (cur) chunks.push(cur);
  return chunks.length ? chunks : [String(content || '')];
}

function toVectorLiteral(values) {
  return `[${values.map((v) => Number(v).toFixed(8)).join(',')}]`;
}

async function rpc(pathname, { method = 'GET', token, body, headers = {} } = {}) {
  const merged = { accept: 'application/json', ...headers };
  if (body !== undefined) merged['content-type'] = 'application/json';
  if (token) merged.authorization = `Bearer ${token}`;
  const res = await fetch(`${REMOTE_URL}${pathname}`, {
    method,
    headers: merged,
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
  const raw = await res.text();
  let json;
  try {
    json = raw ? JSON.parse(raw) : {};
  } catch {
    json = { raw };
  }
  return { ok: res.ok, status: res.status, json, raw };
}

function withAuthLock(fn) {
  const next = authLock.then(fn, fn);
  authLock = next.catch(() => {});
  return next;
}

async function login() {
  const result = await rpc('/v1/auth/local/login', {
    method: 'POST',
    body: { email: SVC_EMAIL, password: SVC_PASSWORD },
  });
  if (!result.ok) {
    throw new Error(`service account login failed: HTTP ${result.status} ${result.raw}`);
  }
  tokens = {
    access: result.json.access_token,
    refresh: result.json.refresh_token,
  };
}

async function refresh() {
  if (!tokens.refresh) return login();
  const result = await rpc('/v1/tokens/refresh', {
    method: 'POST',
    body: { refresh_token: tokens.refresh },
  });
  if (!result.ok) return login();
  tokens = {
    access: result.json.access_token,
    refresh: result.json.refresh_token,
  };
}

async function authed(pathname, opts = {}) {
  if (!tokens.access) await withAuthLock(() => (tokens.access ? Promise.resolve() : login()));
  let res = await rpc(pathname, { ...opts, token: tokens.access });
  if (res.status === 401) {
    await withAuthLock(refresh);
    res = await rpc(pathname, { ...opts, token: tokens.access });
  }
  return res;
}

// Ensure the configured Ollama model is present, pulling it if missing.
// Idempotent; safe to call repeatedly. Resolves true when the model is ready.
async function ensureOllamaModel() {
  if (EMBED_PROVIDER !== 'ollama') {
    ollamaModelReady = true;
    return true;
  }
  try {
    const tagsRes = await fetch(`${EMBED_BASE_URL}/api/tags`);
    if (tagsRes.ok) {
      const tags = await tagsRes.json();
      const has = (tags.models || []).some(
        (m) => (m.name || '').split(':')[0] === EMBED_MODEL.split(':')[0]
      );
      if (has) {
        ollamaModelReady = true;
        return true;
      }
    }
    log(`pulling embed model "${EMBED_MODEL}" (one-time)...`);
    const res = await fetch(`${EMBED_BASE_URL}/api/pull`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ name: EMBED_MODEL, stream: false }),
    });
    if (!res.ok) {
      log(`model pull failed: HTTP ${res.status} ${await res.text()}`);
      return false;
    }
    ollamaModelReady = true;
    log(`embed model "${EMBED_MODEL}" ready`);
    return true;
  } catch (error) {
    log(`ensureOllamaModel error: ${error.message}`);
    return false;
  }
}

async function ollamaEmbed(texts, pulledOnce = false) {
  // Ollama /api/embed accepts an array input and returns embeddings in order.
  const res = await fetch(`${EMBED_BASE_URL}/api/embed`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ model: EMBED_MODEL, input: texts }),
  });
  if (res.ok) {
    const j = await res.json();
    if (Array.isArray(j.embeddings) && j.embeddings.length === texts.length) return j.embeddings;
    if (j.embedding) return [j.embedding];
    throw new Error('ollama embed response missing embeddings');
  }
  const errText = await res.text();
  // Model not present yet → pull once and retry (self-healing first run).
  if (!pulledOnce && /not found|try pulling|no such model/i.test(errText)) {
    await ensureOllamaModel();
    return ollamaEmbed(texts, true);
  }
  throw new Error(`ollama embed failed: ${res.status} ${errText}`);
}

async function embedBatch(texts) {
  if (!texts.length) return [];
  switch (EMBED_PROVIDER) {
    case 'ollama':
      return ollamaEmbed(texts);
    case 'http': {
      const res = await fetch(`${EMBED_BASE_URL}/embed`, {
        method: 'POST',
        headers: { 'content-type': 'application/json', ...(EMBED_API_KEY ? { authorization: `Bearer ${EMBED_API_KEY}` } : {}) },
        body: JSON.stringify({ input: texts, model: EMBED_MODEL }),
      });
      if (!res.ok) throw new Error(`http embed failed: ${res.status} ${await res.text()}`);
      const json = await res.json();
      return json.embeddings || [];
    }
    case 'openai': {
      const res = await fetch('https://api.openai.com/v1/embeddings', {
        method: 'POST',
        headers: {
          'content-type': 'application/json',
          authorization: `Bearer ${EMBED_API_KEY}`,
        },
        body: JSON.stringify({ model: EMBED_MODEL || 'text-embedding-3-small', input: texts }),
      });
      if (!res.ok) throw new Error(`openai embed failed: ${res.status} ${await res.text()}`);
      const json = await res.json();
      return (json.data || []).map((x) => x.embedding);
    }
    case 'voyage': {
      const res = await fetch('https://api.voyageai.com/v1/embeddings', {
        method: 'POST',
        headers: {
          'content-type': 'application/json',
          authorization: `Bearer ${EMBED_API_KEY}`,
        },
        body: JSON.stringify({ model: EMBED_MODEL || 'voyage-3', input: texts }),
      });
      if (!res.ok) throw new Error(`voyage embed failed: ${res.status} ${await res.text()}`);
      const json = await res.json();
      return (json.data || []).map((x) => x.embedding);
    }
    case 'cohere': {
      const res = await fetch('https://api.cohere.com/v2/embed', {
        method: 'POST',
        headers: {
          'content-type': 'application/json',
          authorization: `Bearer ${EMBED_API_KEY}`,
        },
        body: JSON.stringify({ model: EMBED_MODEL || 'embed-english-v3.0', texts, input_type: 'search_document' }),
      });
      if (!res.ok) throw new Error(`cohere embed failed: ${res.status} ${await res.text()}`);
      const json = await res.json();
      return json.embeddings?.float || [];
    }
    default:
      throw new Error(`unsupported EMBED_PROVIDER "${EMBED_PROVIDER}"`);
  }
}

// Embed a batch with bounded exponential backoff (§3.5 resilience). Transient
// embedder failures retry instead of immediately degrading ingestion.
async function embedBatchWithRetry(slice) {
  let lastError;
  for (let attempt = 1; attempt <= EMBED_MAX_RETRIES; attempt += 1) {
    try {
      return await embedBatch(slice);
    } catch (error) {
      lastError = error;
      if (attempt === EMBED_MAX_RETRIES) break;
      const backoff = Math.min(500 * 2 ** (attempt - 1), 8000);
      log(`embed attempt ${attempt}/${EMBED_MAX_RETRIES} failed: ${error.message}; retry in ${backoff}ms`);
      await sleep(backoff);
    }
  }
  throw lastError;
}

async function embedTexts(texts) {
  const all = [];
  for (let i = 0; i < texts.length; i += EMBED_BATCH) {
    const slice = texts.slice(i, i + EMBED_BATCH);
    const vectors = await embedBatchWithRetry(slice);
    for (const vector of vectors) {
      if (!Array.isArray(vector) || vector.length !== EMBED_DIMENSIONS) {
        throw new Error(`embedding dimension mismatch: expected ${EMBED_DIMENSIONS}`);
      }
      all.push(vector);
    }
  }
  return all;
}

async function runMigrations() {
  await db.query(`
    CREATE TABLE IF NOT EXISTS schema_migrations (
      name text PRIMARY KEY,
      applied_at timestamptz NOT NULL DEFAULT now()
    )
  `);
  const migrationDir = path.join(__dirname, 'migrations');
  const files = fs.readdirSync(migrationDir).filter((x) => x.endsWith('.sql')).sort();
  for (const file of files) {
    const check = await db.query('SELECT 1 FROM schema_migrations WHERE name = $1', [file]);
    if (check.rowCount) continue;
    const sqlPath = path.join(migrationDir, file);
    const raw = fs.readFileSync(sqlPath, 'utf8');
    const sql = raw.replaceAll('__EMBED_DIMENSIONS__', String(EMBED_DIMENSIONS));
    await db.query('BEGIN');
    try {
      await db.query(sql);
      await db.query('INSERT INTO schema_migrations(name) VALUES ($1)', [file]);
      await db.query('COMMIT');
      log(`applied migration ${file}`);
    } catch (error) {
      await db.query('ROLLBACK');
      throw error;
    }
  }
}

async function writeAudit(actor, action, recordId, sourceRef, details = {}) {
  await db.query(
    `INSERT INTO memory_audit(id, actor, action, record_id, source_ref, details)
     VALUES ($1, $2, $3, $4, $5, $6::jsonb)`,
    [randomUUID(), actor, action, recordId, sourceRef, JSON.stringify(details)]
  );
}

async function upsertMemoryRecord(record, actor = 'system') {
  const values = [
    record.id || randomUUID(),
    record.org_id,
    record.project_id || null,
    record.source_type,
    record.source_ref,
    record.title || '',
    record.summary || '',
    record.content || '',
    record.content_hash || crypto.createHash('sha256').update(record.content || '').digest('hex'),
    record.tags || [],
    record.actors || [],
    record.visibility || 'org',
    record.confidence ?? 0.5,
    record.embed_model || '',
    record.embed_dim || 0,
    record.embedding ? toVectorLiteral(record.embedding) : null,
  ];
  const result = await db.query(
    `INSERT INTO memory_records(
      id, org_id, project_id, source_type, source_ref, title, summary, content,
      content_hash, tags, actors, visibility, confidence, embed_model, embed_dim, embedding
    ) VALUES (
      $1, $2, $3, $4, $5, $6, $7, $8,
      $9, $10, $11, $12, $13, $14, $15, $16::vector
    )
    ON CONFLICT (org_id, source_ref)
    DO UPDATE SET
      project_id = EXCLUDED.project_id,
      source_type = EXCLUDED.source_type,
      title = EXCLUDED.title,
      summary = EXCLUDED.summary,
      content = EXCLUDED.content,
      content_hash = EXCLUDED.content_hash,
      tags = EXCLUDED.tags,
      actors = EXCLUDED.actors,
      visibility = EXCLUDED.visibility,
      confidence = EXCLUDED.confidence,
      embed_model = EXCLUDED.embed_model,
      embed_dim = EXCLUDED.embed_dim,
      embedding = EXCLUDED.embedding,
      updated_at = now(),
      deleted_at = NULL
    RETURNING id, source_ref`,
    values
  );
  const row = result.rows[0];
  await writeAudit(actor, 'write', row.id, row.source_ref, { source_type: record.source_type });
  return row;
}

async function searchMemory({
  org_id,
  project_id,
  source_type,
  query,
  top_k = 10,
}) {
  if (!org_id) throw new Error('org_id is required');
  if (!query || !String(query).trim()) throw new Error('query is required');
  const args = [org_id];
  const filters = ['org_id = $1', 'deleted_at IS NULL'];
  if (project_id) {
    args.push(project_id);
    filters.push(`(project_id = $${args.length} OR project_id IS NULL)`);
  }
  if (source_type) {
    args.push(source_type);
    filters.push(`source_type = $${args.length}`);
  }
  const whereSql = filters.join(' AND ');

  try {
    const [vector] = await embedTexts([query]);
    args.push(toVectorLiteral(vector));
    args.push(Math.max(1, Math.min(50, Number(top_k) || 10)));
    const sql = `
      SELECT
        id, org_id, project_id, source_type, source_ref, title, summary, tags, actors,
        visibility, confidence, updated_at,
        1 - (embedding <=> $${args.length - 1}::vector) AS score
      FROM memory_records
      WHERE ${whereSql} AND embedding IS NOT NULL
      ORDER BY embedding <=> $${args.length - 1}::vector
      LIMIT $${args.length}
    `;
    const result = await db.query(sql, args);
    return { degraded: false, results: result.rows };
  } catch (error) {
    const fallbackArgs = [...args, query, Math.max(1, Math.min(50, Number(top_k) || 10))];
    const sql = `
      SELECT
        id, org_id, project_id, source_type, source_ref, title, summary, tags, actors,
        visibility, confidence, updated_at,
        ts_rank(content_tsv, plainto_tsquery('english', $${fallbackArgs.length - 1})) AS score
      FROM memory_records
      WHERE ${whereSql}
        AND content_tsv @@ plainto_tsquery('english', $${fallbackArgs.length - 1})
      ORDER BY score DESC, updated_at DESC
      LIMIT $${fallbackArgs.length}
    `;
    const result = await db.query(sql, fallbackArgs);
    return { degraded: true, reason: error.message, results: result.rows };
  }
}

async function runbookLookup({ org_id, project_id, topic, top_k = 5 }) {
  return searchMemory({
    org_id,
    project_id,
    source_type: 'runbook',
    query: topic,
    top_k,
  });
}

async function listProjects(orgId) {
  const r = await authed(`/v1/projects?organization_id=${encodeURIComponent(orgId)}`);
  if (!r.ok) throw new Error(`list projects failed: HTTP ${r.status}`);
  return r.json.projects || r.json.data || [];
}

async function searchIssues(projectId, limit = 100) {
  const body = {
    project_id: projectId,
    sort_field: 'updated_at',
    sort_direction: 'desc',
    limit,
    offset: 0,
  };
  const r = await authed('/v1/issues/search', { method: 'POST', body });
  if (!r.ok) throw new Error(`search issues failed: HTTP ${r.status}`);
  return r.json.issues || r.json.data || [];
}

async function issueContext({ issue_id }) {
  if (!issue_id) throw new Error('issue_id is required');
  const [issueRes, commentsRes, assigneesRes] = await Promise.all([
    authed(`/v1/issues/${encodeURIComponent(issue_id)}`),
    authed(`/v1/issue_comments?issue_id=${encodeURIComponent(issue_id)}`),
    authed(`/v1/issue_assignees?issue_id=${encodeURIComponent(issue_id)}`),
  ]);
  if (!issueRes.ok) throw new Error(`issue lookup failed: HTTP ${issueRes.status}`);
  return {
    issue: issueRes.json.data || issueRes.json,
    comments: commentsRes.ok ? commentsRes.json.issue_comments || commentsRes.json.data || [] : [],
    assignees: assigneesRes.ok ? assigneesRes.json.issue_assignees || assigneesRes.json.data || [] : [],
  };
}

async function projectBrief({ project_id }) {
  if (!project_id) throw new Error('project_id is required');
  const [projectRes, statusesRes, issues] = await Promise.all([
    authed(`/v1/projects/${encodeURIComponent(project_id)}`),
    authed(`/v1/project_statuses?project_id=${encodeURIComponent(project_id)}`),
    searchIssues(project_id, 200),
  ]);
  if (!projectRes.ok) throw new Error(`project lookup failed: HTTP ${projectRes.status}`);
  const rows = issues || [];
  const openIssues = rows.filter((x) => !x.completed_at).length;
  const closedIssues = rows.filter((x) => !!x.completed_at).length;
  return {
    project: projectRes.json.data || projectRes.json,
    statuses: statusesRes.ok ? statusesRes.json.project_statuses || statusesRes.json.data || [] : [],
    metrics: {
      total_issues: rows.length,
      open_issues: openIssues,
      closed_issues: closedIssues,
      recently_updated: rows.slice(0, 10),
    },
  };
}

async function recentChanges({ org_id, since, limit = 200 }) {
  if (!org_id) throw new Error('org_id is required');
  const sinceTs = since ? new Date(since).getTime() : Date.now() - 24 * 3600 * 1000;
  const projects = await listProjects(org_id);
  const all = [];
  for (const project of projects) {
    const issues = await searchIssues(project.id, 100);
    for (const issue of issues) {
      const updatedAt = issue.updated_at ? new Date(issue.updated_at).getTime() : 0;
      if (updatedAt >= sinceTs) {
        all.push({
          issue_id: issue.id,
          project_id: project.id,
          project_name: project.name,
          simple_id: issue.simple_id,
          title: issue.title,
          updated_at: issue.updated_at,
          status_id: issue.status_id,
          assignee_user_id: issue.assignee_user_id || null,
          source_ref: `issue:${issue.id}`,
        });
      }
    }
  }
  all.sort((a, b) => new Date(b.updated_at) - new Date(a.updated_at));
  return { changes: all.slice(0, Math.max(1, Math.min(1000, Number(limit) || 200))) };
}

async function ingestDocs({ org_id = MEMORY_DEFAULT_ORG_ID, project_id = null, actor = 'memory-ingest' }) {
  if (!org_id) throw new Error('org_id is required');
  const allDocs = [];
  for (const rel of DOC_INCLUDE) {
    const full = path.join(DOC_ROOT, rel);
    if (!fs.existsSync(full) || !fs.statSync(full).isFile()) continue;
    const raw = fs.readFileSync(full, 'utf8');
    const redacted = redactSecrets(raw);
    const chunks = chunkText(redacted);
    for (let i = 0; i < chunks.length; i += 1) {
      allDocs.push({
        source_ref: `doc:${rel}#${i + 1}`,
        title: path.basename(rel),
        summary: chunks[i].slice(0, 280),
        content: chunks[i],
        content_hash: crypto.createHash('sha256').update(chunks[i]).digest('hex'),
        source_type: inferSourceType(rel),
        tags: ['docs', rel.toLowerCase()],
      });
    }
  }
  // Idempotency (§6): skip chunks whose content_hash already matches a stored
  // record, so re-running only re-embeds changed docs.
  let existing = { rows: [] };
  if (allDocs.length) {
    existing = await db.query(
      `SELECT source_ref, content_hash FROM memory_records
       WHERE org_id = $1 AND source_ref = ANY($2) AND deleted_at IS NULL`,
      [org_id, allDocs.map((d) => d.source_ref)]
    );
  }
  const existingHash = new Map(existing.rows.map((r) => [r.source_ref, r.content_hash]));
  const docs = allDocs.filter((d) => existingHash.get(d.source_ref) !== d.content_hash);
  const skipped = allDocs.length - docs.length;
  let embeddings = [];
  let degraded = false;
  try {
    embeddings = await embedTexts(docs.map((x) => x.content));
  } catch (error) {
    degraded = true;
    log(`embedder unavailable during docs ingest; storing keyword-only records (${error.message})`);
    embeddings = new Array(docs.length).fill(null);
  }
  let written = 0;
  for (let i = 0; i < docs.length; i += 1) {
    await upsertMemoryRecord({
      id: randomUUID(),
      org_id,
      project_id,
      source_type: docs[i].source_type,
      source_ref: docs[i].source_ref,
      title: docs[i].title,
      summary: docs[i].summary,
      content: docs[i].content,
      content_hash: docs[i].content_hash,
      tags: docs[i].tags,
      actors: [],
      visibility: project_id ? 'project' : 'org',
      confidence: degraded ? 0.45 : 0.8,
      embed_model: degraded ? '' : `${EMBED_PROVIDER}:${EMBED_MODEL}`,
      embed_dim: degraded ? 0 : EMBED_DIMENSIONS,
      embedding: embeddings[i],
    }, actor);
    written += 1;
  }
  return { ingested: written, skipped, docs: DOC_INCLUDE, degraded };
}

function resolutionTextCandidates(comments) {
  if (!comments.length) return [];
  const re = /(resolved|resolution|root cause|rca|fixed|workaround|closed)/i;
  const preferred = comments.filter((c) => re.test(c.message || ''));
  return preferred.length ? preferred : [comments[comments.length - 1]];
}

async function ingestIssueNotes({
  org_id = MEMORY_DEFAULT_ORG_ID,
  actor = 'memory-ingest',
  limit_per_project = 50,
}) {
  if (!org_id) throw new Error('org_id is required');
  const projects = await listProjects(org_id);
  let ingested = 0;
  for (const project of projects) {
    const issues = await searchIssues(project.id, limit_per_project);
    for (const issue of issues) {
      if (!issue.completed_at) continue;
      const commentsRes = await authed(`/v1/issue_comments?issue_id=${encodeURIComponent(issue.id)}`);
      if (!commentsRes.ok) continue;
      const comments = commentsRes.json.issue_comments || commentsRes.json.data || [];
      const candidates = resolutionTextCandidates(comments);
      for (const comment of candidates) {
        const content = redactSecrets(comment.message || '');
        if (!content.trim()) continue;
        let embedding = null;
        let degraded = false;
        try {
          [embedding] = await embedTexts([content]);
        } catch {
          degraded = true;
        }
        await upsertMemoryRecord({
          id: randomUUID(),
          org_id,
          project_id: project.id,
          source_type: 'issue_note',
          source_ref: `issue:${issue.id}:comment:${comment.id}`,
          title: `${issue.simple_id || issue.id} ${issue.title || ''}`.trim(),
          summary: content.slice(0, 280),
          content,
          tags: ['issue_note', project.name || '', issue.simple_id || ''],
          actors: [String(comment.user_id || '')].filter(Boolean),
          visibility: 'project',
          confidence: degraded ? 0.4 : 0.75,
          embed_model: degraded ? '' : `${EMBED_PROVIDER}:${EMBED_MODEL}`,
          embed_dim: degraded ? 0 : EMBED_DIMENSIONS,
          embedding,
        }, actor);
        ingested += 1;
      }
    }
  }
  return { ingested, org_id };
}

async function reembedAll({ actor = 'memory-reembed', only_missing = false }) {
  // only_missing heals rows ingested while the embedder was down (embedding IS NULL)
  // without re-embedding the whole corpus.
  const where = only_missing
    ? 'WHERE deleted_at IS NULL AND embedding IS NULL'
    : 'WHERE deleted_at IS NULL';
  const rows = await db.query(
    `SELECT id, source_ref, content FROM memory_records ${where} ORDER BY updated_at ASC`
  );
  let updated = 0;
  for (const row of rows.rows) {
    const [vector] = await embedTexts([row.content]);
    await db.query(
      `UPDATE memory_records
       SET embedding = $1::vector, embed_model = $2, embed_dim = $3, updated_at = now()
       WHERE id = $4`,
      [toVectorLiteral(vector), `${EMBED_PROVIDER}:${EMBED_MODEL}`, EMBED_DIMENSIONS, row.id]
    );
    await writeAudit(actor, 'reembed', row.id, row.source_ref, {
      embed_model: `${EMBED_PROVIDER}:${EMBED_MODEL}`,
      embed_dim: EMBED_DIMENSIONS,
    });
    updated += 1;
  }
  return { reembedded: updated };
}

// Retention (§7): soft-delete noisy types past the noisy window, then purge
// rows that have been soft-deleted past the purge window. Curated types
// (runbook/decision/incident) are kept indefinitely.
async function retentionSweep({ actor = 'memory-retention' } = {}) {
  const soft = await db.query(
    `UPDATE memory_records SET deleted_at = now(), updated_at = now()
     WHERE deleted_at IS NULL
       AND source_type = ANY($1)
       AND created_at < now() - make_interval(days => $2)
     RETURNING id, source_ref`,
    [RETENTION_NOISY_TYPES, RETENTION_NOISY_DAYS]
  );
  for (const row of soft.rows) {
    await writeAudit(actor, 'delete', row.id, row.source_ref, { reason: 'retention-soft', days: RETENTION_NOISY_DAYS });
  }
  const purge = await db.query(
    `DELETE FROM memory_records
     WHERE deleted_at IS NOT NULL
       AND deleted_at < now() - make_interval(days => $1)
     RETURNING id, source_ref`,
    [RETENTION_PURGE_DAYS]
  );
  for (const row of purge.rows) {
    await writeAudit(actor, 'delete', row.id, row.source_ref, { reason: 'retention-purge', days: RETENTION_PURGE_DAYS });
  }
  return { soft_deleted: soft.rowCount, purged: purge.rowCount };
}

function send(res, status, obj) {
  res.writeHead(status, { 'content-type': 'application/json; charset=utf-8' });
  res.end(JSON.stringify(obj));
}

function readBody(req, limit = 512 * 1024) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    let size = 0;
    req.on('data', (chunk) => {
      size += chunk.length;
      if (size > limit) {
        req.destroy();
        reject(new Error('payload too large'));
        return;
      }
      chunks.push(chunk);
    });
    req.on('end', () => {
      const raw = Buffer.concat(chunks).toString('utf8');
      if (!raw) return resolve({});
      try {
        resolve(JSON.parse(raw));
      } catch {
        reject(new Error('invalid JSON'));
      }
    });
    req.on('error', reject);
  });
}

function checkApiKey(req) {
  const hdr = req.headers['x-api-key'] ||
    String(req.headers.authorization || '').replace(/^Bearer\s+/i, '');
  return API_KEY && hdr && crypto.timingSafeEqual(Buffer.from(String(hdr)), Buffer.from(String(API_KEY)));
}

async function handleApi(req, res, pathname) {
  if ((pathname === '/health' || pathname === '/memory/health') && req.method === 'GET') {
    return send(res, 200, { status: 'ok', provider: EMBED_PROVIDER, model: EMBED_MODEL });
  }
  if (!checkApiKey(req)) return send(res, 401, { error: 'unauthorized' });
  const body = await readBody(req);
  // Org isolation enforced in the service (§7): the shared key may only act on
  // permitted org(s). Cross-org requests are rejected, not silently scoped.
  if (ALLOWED_ORG_IDS.length && body && body.org_id && !ALLOWED_ORG_IDS.includes(body.org_id)) {
    return send(res, 403, { error: 'org_id not permitted for this API key' });
  }
  try {
    if (pathname === '/memory/retention' && req.method === 'POST') {
      return send(res, 200, await retentionSweep(body));
    }
    if (pathname === '/memory/search' && req.method === 'POST') {
      return send(res, 200, await searchMemory(body));
    }
    if (pathname === '/memory/runbook' && req.method === 'POST') {
      return send(res, 200, await runbookLookup({
        org_id: body.org_id,
        project_id: body.project_id || null,
        topic: body.topic || body.query,
        top_k: body.top_k || 5,
      }));
    }
    if (pathname === '/memory/recent_changes' && req.method === 'POST') {
      return send(res, 200, await recentChanges(body));
    }
    if (pathname === '/memory/issue_context' && req.method === 'POST') {
      return send(res, 200, await issueContext(body));
    }
    if (pathname === '/memory/project_brief' && req.method === 'POST') {
      return send(res, 200, await projectBrief(body));
    }
    if (pathname === '/memory/ingest/docs' && req.method === 'POST') {
      return send(res, 200, await ingestDocs(body));
    }
    if (pathname === '/memory/ingest/issues' && req.method === 'POST') {
      return send(res, 200, await ingestIssueNotes(body));
    }
    if (pathname === '/memory/reembed' && req.method === 'POST') {
      return send(res, 200, await reembedAll(body));
    }
    return send(res, 404, { error: 'not found' });
  } catch (error) {
    log(`ERROR ${pathname}: ${error.message}`);
    return send(res, 500, { error: error.message });
  }
}

function mcpTools() {
  return [
    {
      name: 'memory.search',
      description: 'Semantic org/project memory search with citations.',
      inputSchema: {
        type: 'object',
        properties: {
          org_id: { type: 'string' },
          project_id: { type: 'string' },
          query: { type: 'string' },
          top_k: { type: 'number' },
        },
        required: ['org_id', 'query'],
      },
    },
    {
      name: 'memory.recent_changes',
      description: 'Live issue changes since timestamp from Vibe Kanban API.',
      inputSchema: {
        type: 'object',
        properties: {
          org_id: { type: 'string' },
          since: { type: 'string' },
          limit: { type: 'number' },
        },
        required: ['org_id', 'since'],
      },
    },
    {
      name: 'memory.runbook',
      description: 'Lookup runbook entries for a topic.',
      inputSchema: {
        type: 'object',
        properties: {
          org_id: { type: 'string' },
          project_id: { type: 'string' },
          topic: { type: 'string' },
          top_k: { type: 'number' },
        },
        required: ['org_id', 'topic'],
      },
    },
    {
      name: 'memory.issue_context',
      description: 'Fetch issue details, comments, and assignees live.',
      inputSchema: {
        type: 'object',
        properties: { issue_id: { type: 'string' } },
        required: ['issue_id'],
      },
    },
    {
      name: 'memory.project_brief',
      description: 'Fetch project summary and issue metrics live.',
      inputSchema: {
        type: 'object',
        properties: { project_id: { type: 'string' } },
        required: ['project_id'],
      },
    },
  ];
}

async function executeTool(name, args) {
  switch (name) {
    case 'memory.search':
      return searchMemory(args || {});
    case 'memory.recent_changes':
      return recentChanges(args || {});
    case 'memory.runbook':
      return runbookLookup(args || {});
    case 'memory.issue_context':
      return issueContext(args || {});
    case 'memory.project_brief':
      return projectBrief(args || {});
    default:
      throw new Error(`unknown tool: ${name}`);
  }
}

async function runMcpStdio() {
  log('starting MCP stdio mode');
  const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
  for await (const line of rl) {
    if (!line.trim()) continue;
    let msg;
    try {
      msg = JSON.parse(line);
    } catch {
      continue;
    }
    const id = Object.prototype.hasOwnProperty.call(msg, 'id') ? msg.id : null;
    try {
      if (msg.method === 'initialize') {
        process.stdout.write(JSON.stringify({
          jsonrpc: '2.0',
          id,
          result: {
            serverInfo: { name: 'vibe-kanban-memory', version: '0.1.0' },
            capabilities: { tools: {} },
          },
        }) + '\n');
      } else if (msg.method === 'tools/list') {
        process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id, result: { tools: mcpTools() } }) + '\n');
      } else if (msg.method === 'tools/call') {
        const name = msg.params?.name;
        const args = msg.params?.arguments || {};
        const result = await executeTool(name, args);
        process.stdout.write(JSON.stringify({
          jsonrpc: '2.0',
          id,
          result: {
            content: [{ type: 'text', text: JSON.stringify(result) }],
          },
        }) + '\n');
      } else if (id !== null) {
        process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id, result: {} }) + '\n');
      }
    } catch (error) {
      if (id !== null) {
        process.stdout.write(JSON.stringify({
          jsonrpc: '2.0',
          id,
          error: { code: -32000, message: error.message },
        }) + '\n');
      }
    }
  }
}

async function main() {
  await db.connect();
  await runMigrations();
  if (process.argv.includes('--mcp-stdio')) {
    await runMcpStdio();
    return;
  }
  const server = http.createServer(async (req, res) => {
    let pathname = req.url || '/';
    try {
      pathname = new URL(req.url, 'http://localhost').pathname;
    } catch {
      // keep raw
    }
    if (pathname.length > 1) pathname = pathname.replace(/\/+$/, '');
    return handleApi(req, res, pathname);
  });
  server.listen(PORT, () => {
    log(`listening on :${PORT} provider=${EMBED_PROVIDER} model=${EMBED_MODEL}`);
  });
  // Warm up the embed model in the background so the first ingest/search isn't
  // forced onto the degraded keyword path (gap #1).
  ensureOllamaModel().then((ok) => {
    if (!ok) log('embed model not ready yet; will pull on first embed call');
  });
}

main().catch((error) => {
  console.error(`[memory] FATAL: ${error.message}`);
  process.exit(1);
});
