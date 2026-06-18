-- Track the user who created each workspace locally.
ALTER TABLE workspaces ADD COLUMN owner_user_id TEXT;
