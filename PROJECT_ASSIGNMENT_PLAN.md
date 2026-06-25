# Project Assignment + Personal/Team Tabs — Implementation Plan

**Status:** Design (no code yet) · **Author:** Sakib · **Date:** 2026-06-25

## Goal

Let an **admin assign whole projects to specific team members** from
`vk.rokomari.io`. In the local launcher, each user gets:

- **Personal tab** — only the projects assigned to me (I can open and work on
  their issues normally).
- **Team tab** — all projects and everyone's issues (today's behaviour, unchanged).

### Visibility model (decided): **Filter-only**

Members still sync all org data and can see everything via the **Team** tab.
Project assignment is a *convenience filter* that powers the **Personal** tab — it
is **not** an access restriction. This means:

- ✅ No per-project access enforcement on the backend.
- ✅ No re-scoping of Electric sync per user (the big cost we avoid).
- ✅ Admins see everything regardless (their Personal tab shows projects assigned
  to them, which may be empty).

If we ever want a hard access boundary later, it's a separate, larger project
(per-user Electric shapes + access checks). This plan does **not** do that.

---

## 1. Data model

New many-to-many table. A project can be assigned to many members; a member can
be assigned many projects.

```sql
-- crates/remote/migrations/<ts>_project_members.sql
CREATE TABLE project_members (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id  UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    user_id     UUID NOT NULL REFERENCES users(id)    ON DELETE CASCADE,
    assigned_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    assigned_by UUID REFERENCES users(id) ON DELETE SET NULL,   -- audit: which admin
    UNIQUE (project_id, user_id)
);
CREATE INDEX idx_project_members_user_id    ON project_members(user_id);
CREATE INDEX idx_project_members_project_id ON project_members(project_id);

-- Register for Electric sync so the launcher can filter the Personal tab live.
-- Same helper issue_assignees uses (migration 20260114000000_electric_sync_tables.sql).
SELECT electric_sync_table('public', 'project_members');
```

Mirrors the existing `issue_assignees` table (`20260112000000_remote-projects.sql:124`).
Migrations run automatically on `remote` startup, so this applies on deploy.

---

## 2. Backend API (`crates/remote/src/routes/projects.rs` + new db module)

All mutations are **admin-gated** (`ensure_admin_access`, as used in
`organization_members.rs`). Reads use `ensure_member_access`.

| Method | Path | Who | Purpose |
|---|---|---|---|
| `GET` | `/v1/projects/{id}/members` | member | list users assigned to a project |
| `PUT` | `/v1/projects/{id}/members` | **admin** | set the full assignee list `{ user_ids: [..] }` (idempotent replace) |
| `POST` | `/v1/projects/{id}/members` | **admin** | add one/many `{ user_ids: [..] }` |
| `DELETE` | `/v1/projects/{id}/members/{user_id}` | **admin** | unassign one |

`PUT` (replace-set) is the primary path the admin multi-select uses; `POST`/`DELETE`
are convenience. Validate that every `user_id` is a member of the project's org
before inserting (reuse the membership check).

**"My projects" filter** — for non-sync callers, extend the existing list endpoint:

```
GET /v1/projects?organization_id=..&assigned_to_me=true
```

When `assigned_to_me=true`, join `project_members` on `ctx.user.id`
(`db/projects.rs:110 list_by_organization` → add `list_assigned_to_user`).
The launcher itself will mostly rely on Electric sync (below), but this keeps the
REST surface consistent and is handy for the ingest/automation paths.

New db module: `crates/remote/src/db/project_members.rs` (assign, unassign,
replace_set, list_by_project, list_project_ids_for_user).

---

## 3. Types

Two new api-types in `crates/api-types/src/` (e.g. `project_member.rs`):
`ProjectMember { id, project_id, user_id, assigned_at }` and request bodies
`SetProjectMembersRequest { user_ids: Vec<Uuid> }`.

- Add `project_members` to the Electric/shared TS types in
  `shared/remote-types.ts` and define a `PROJECT_MEMBERS_SHAPE` next to
  `PROJECTS_SHAPE`.
- For the REST request/response types, either derive `ts-rs` `TS` and register in
  `bin/generate_types.rs` (then `pnpm run remote:generate-types`), or hand-write
  in web-core (the insights feature hand-wrote — same call here).

---

## 4. Admin UI — assign projects (vk.rokomari.io)

Extend **`RemoteProjectsSettingsSection.tsx`**
(`packages/web-core/src/shared/dialogs/settings/settings/RemoteProjectsSettingsSection.tsx:77`).

- Per project row, add an **"Assigned developers"** control: a member multi-select
  reusing the picker pattern from
  **`AssigneeSelectionDialog.tsx`** (`shared/dialogs/kanban/`) — it already lists
  org members and is admin-gated.
- On save → `PUT /v1/projects/{id}/members`.
- Admin-only: hide/disable for non-admins (the section is already in the admin
  settings area; gate the same way member-role editing is gated).

Members list comes from the existing `GET /v1/organizations/{org_id}/members`.

---

## 5. Launcher UI — Personal / Team tabs

Touch **`SharedAppLayout.tsx`**
(`packages/web-core/src/shared/components/ui-new/containers/SharedAppLayout.tsx:57`,
project list built at `:121-136`).

- Add a **Personal | Team** toggle at the top of the project nav (both always
  visible to every user — filter model).
- **Team** = current behaviour (all org projects via `PROJECTS_SHAPE`).
- **Personal** = same project list filtered to ids where the current user appears
  in `project_members`. Source the assignment set from the new
  `PROJECT_MEMBERS_SHAPE` (Electric, scoped by org), filtered to `user_id == me`.
- Inside a Personal project, the kanban (`pages/kanban/ProjectKanban.tsx:100`) is
  unchanged — the whole project is "mine", so I see all its issues and can work on
  them. (Issue-level `assignee_user_id` filter already exists if we later want a
  "just my issues" sub-filter.)
- Default tab: **Personal** if the user has ≥1 assigned project, else **Team**
  (so unassigned/admin users aren't dropped into an empty view).

No new sync scopes — `project_members` rides the existing org-scoped Electric
publication.

---

## 6. Testing

- **Backend:** unit/integration on the new routes — admin can assign/replace/unassign;
  non-admin gets 403; cross-org `user_id` rejected; `assigned_to_me` filter returns
  only assigned projects.
- **Local smoke (Docker, see deployment notes):** `docker compose build remote` →
  `up -d postgres remote` → login → `PUT` members → `GET ?assigned_to_me=true`.
- **UI:** assign a project to a non-admin test user; confirm it appears in their
  Personal tab and still appears in Team for everyone.

---

## 7. Rollout / deploy

Same flow we used for insights (recorded in deployment notes):

1. Build + smoke-test locally.
2. `rsync --checksum` source → server `vibe-kanban/` vendored copy.
3. **Detached** build on server (`nohup ./build.sh &`) — the new migration applies
   on `remote` startup.
4. `docker compose up -d remote`; verify health + that assignment works on
   `vk.rokomari.io`.
5. Back up the current image first (`docker tag … bak-<date>`) for rollback.

**Migration safety:** additive only (new table + index + publication add). No
changes to existing tables → trivially backward-compatible; old clients ignore the
new tab/table.

---

## 8. Effort estimate

| Area | Rough size |
|---|---|
| Migration + db module + types | S |
| Backend routes + tests | M |
| Admin assign UI (extend existing section) | M |
| Launcher Personal/Team tabs | M |
| Wiring `PROJECT_MEMBERS_SHAPE` sync + filter | S–M |

Total: **~2–4 focused days**, no architectural risk (all additive, reuses existing
membership/assignee/sync patterns).

---

## 9. Open questions (non-blocking)

1. **Sort/visibility of unassigned projects in Team tab** — keep flat as today? (assume yes)
2. **Should assigning a project auto-assign its existing issues** to that member
   (write `issue_assignees` rows), or keep project- and issue-assignment fully
   independent? (assume independent — project assignment only drives the Personal filter)
3. **Empty Personal tab copy** — what to show when a member has no assigned projects
   (e.g. "No projects assigned yet — switch to Team").
