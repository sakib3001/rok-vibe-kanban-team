-- Keep issues.completed_at in sync with the "Done" status.
--
-- Completion was only ever tracked via status_id (the completed_at column was
-- never populated), which made accurate cycle-time and time-windowed "done"
-- counts impossible. A BEFORE trigger stamps completed_at whenever an issue is
-- in a Done status and clears it otherwise — robust across every write path
-- (REST mutations, ingest, direct SQL). We then backfill existing Done issues.

CREATE OR REPLACE FUNCTION set_issue_completed_at()
RETURNS TRIGGER AS $$
DECLARE
    status_name TEXT;
BEGIN
    SELECT ps.name INTO status_name
    FROM project_statuses ps
    WHERE ps.id = NEW.status_id;

    IF status_name IS NOT NULL AND lower(status_name) = 'done' THEN
        -- Preserve the first completion time; only set it on entry to Done.
        IF NEW.completed_at IS NULL THEN
            NEW.completed_at := NOW();
        END IF;
    ELSE
        -- Reopened / not done: no completion time.
        NEW.completed_at := NULL;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS trg_issue_completed_at ON issues;
CREATE TRIGGER trg_issue_completed_at
    BEFORE INSERT OR UPDATE ON issues
    FOR EACH ROW
    EXECUTE FUNCTION set_issue_completed_at();

-- Backfill existing Done issues (use updated_at as the best available proxy).
UPDATE issues i
SET completed_at = COALESCE(i.completed_at, i.updated_at)
FROM project_statuses ps
WHERE ps.id = i.status_id
  AND lower(ps.name) = 'done'
  AND i.completed_at IS NULL;
