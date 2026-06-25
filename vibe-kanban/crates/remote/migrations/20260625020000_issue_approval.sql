-- Admin approval gate for completing issues.
--
-- Per-project opt-in: when projects.requires_done_approval is true, moving an
-- issue to a Done status marks it "pending" (NOT complete) until an admin
-- approves. completed_at is only stamped once approved, so insights/cycle-time
-- count only approved work. Enforced in the trigger so no write path bypasses it.

ALTER TABLE projects
    ADD COLUMN IF NOT EXISTS requires_done_approval BOOLEAN NOT NULL DEFAULT FALSE;

ALTER TABLE issues
    ADD COLUMN IF NOT EXISTS approval_state TEXT NOT NULL DEFAULT 'not_required',
    ADD COLUMN IF NOT EXISTS approved_by UUID REFERENCES users(id) ON DELETE SET NULL,
    ADD COLUMN IF NOT EXISTS approved_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS approval_note TEXT;

-- approval_state ∈ {not_required, pending, approved}. ('rejected' is modelled as
-- "back to In Progress with a note", so it is not a persisted state here.)

-- Replace the completion trigger to gate completed_at on approval. The trigger
-- itself (trg_issue_completed_at, BEFORE INSERT OR UPDATE) already exists.
CREATE OR REPLACE FUNCTION set_issue_completed_at()
RETURNS TRIGGER AS $$
DECLARE
    status_name TEXT;
    requires BOOLEAN;
BEGIN
    SELECT ps.name INTO status_name
    FROM project_statuses ps WHERE ps.id = NEW.status_id;
    SELECT p.requires_done_approval INTO requires
    FROM projects p WHERE p.id = NEW.project_id;

    IF status_name IS NOT NULL AND lower(status_name) = 'done' THEN
        IF COALESCE(requires, false) AND NEW.approval_state <> 'approved' THEN
            -- Pending admin approval: not complete yet.
            IF NEW.approval_state = 'not_required' THEN
                NEW.approval_state := 'pending';
            END IF;
            NEW.completed_at := NULL;
        ELSE
            -- Approved, or approval not required: complete.
            IF NEW.completed_at IS NULL THEN
                NEW.completed_at := NOW();
            END IF;
        END IF;
    ELSE
        -- Not done: clear completion + approval workflow fields.
        NEW.completed_at := NULL;
        NEW.approval_state := 'not_required';
        NEW.approved_by := NULL;
        NEW.approved_at := NULL;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;
