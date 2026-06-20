CREATE EXTENSION IF NOT EXISTS vector;

CREATE TABLE IF NOT EXISTS memory_records (
  id uuid PRIMARY KEY,
  org_id uuid NOT NULL,
  project_id uuid NULL,
  source_type text NOT NULL CHECK (
    source_type IN ('runbook', 'decision', 'incident', 'note', 'issue_note', 'dispatch')
  ),
  source_ref text NOT NULL,
  title text NOT NULL,
  summary text NOT NULL DEFAULT '',
  content text NOT NULL,
  content_hash text NOT NULL,
  tags text[] NOT NULL DEFAULT '{}',
  actors text[] NOT NULL DEFAULT '{}',
  visibility text NOT NULL DEFAULT 'org' CHECK (
    visibility IN ('org', 'project', 'team', 'private')
  ),
  confidence real NOT NULL DEFAULT 0.5,
  embed_model text NOT NULL DEFAULT '',
  embed_dim integer NOT NULL DEFAULT 0,
  embedding vector(__EMBED_DIMENSIONS__) NULL,
  content_tsv tsvector GENERATED ALWAYS AS (
    to_tsvector(
      'english',
      coalesce(title, '') || ' ' || coalesce(summary, '') || ' ' || coalesce(content, '')
    )
  ) STORED,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now(),
  deleted_at timestamptz NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_memory_records_org_source_ref
  ON memory_records (org_id, source_ref);

CREATE INDEX IF NOT EXISTS idx_memory_records_org_project
  ON memory_records (org_id, project_id);

CREATE INDEX IF NOT EXISTS idx_memory_records_source_type
  ON memory_records (source_type);

CREATE INDEX IF NOT EXISTS idx_memory_records_deleted_at
  ON memory_records (deleted_at);

CREATE INDEX IF NOT EXISTS idx_memory_records_content_tsv
  ON memory_records USING GIN (content_tsv);

CREATE INDEX IF NOT EXISTS idx_memory_records_embedding
  ON memory_records USING ivfflat (embedding vector_cosine_ops)
  WITH (lists = 100);

CREATE TABLE IF NOT EXISTS memory_audit (
  id uuid PRIMARY KEY,
  ts timestamptz NOT NULL DEFAULT now(),
  actor text NOT NULL,
  action text NOT NULL CHECK (action IN ('write', 'update', 'delete', 'reembed')),
  record_id uuid NULL,
  source_ref text NOT NULL,
  details jsonb NOT NULL DEFAULT '{}'::jsonb
);

CREATE INDEX IF NOT EXISTS idx_memory_audit_ts ON memory_audit (ts DESC);
CREATE INDEX IF NOT EXISTS idx_memory_audit_record ON memory_audit (record_id);
