-- Project assignment: maps organization members to the projects they are
-- assigned to. Powers the launcher's "Personal" tab (projects assigned to me).
-- This is a convenience/visibility filter, NOT an access boundary — every org
-- member still syncs all org data and can use the "Team" view.

CREATE TABLE project_members (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id  UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    user_id     UUID NOT NULL REFERENCES users(id)    ON DELETE CASCADE,
    assigned_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- The admin who made the assignment (audit). NULL if that user is removed.
    assigned_by UUID REFERENCES users(id) ON DELETE SET NULL,
    UNIQUE (project_id, user_id)
);

CREATE INDEX idx_project_members_user_id    ON project_members(user_id);
CREATE INDEX idx_project_members_project_id ON project_members(project_id);

-- Stream assignments to clients so the Personal tab updates live.
-- Same helper used by issue_assignees (20260114000000_electric_sync_tables.sql).
SELECT electric_sync_table('public', 'project_members');
