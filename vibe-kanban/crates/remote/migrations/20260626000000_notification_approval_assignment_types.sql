-- Notification types for the admin approval workflow and project assignment.
-- See migration 20260310000000 for the same ALTER TYPE ... ADD VALUE pattern.
ALTER TYPE notification_type ADD VALUE 'issue_approval_requested';
ALTER TYPE notification_type ADD VALUE 'issue_approval_granted';
ALTER TYPE notification_type ADD VALUE 'issue_approval_rejected';
ALTER TYPE notification_type ADD VALUE 'project_assigned';
