import { useMemo, useState } from "react";
import { Link } from "@tanstack/react-router";
import { useMutation, useQueries, useQuery } from "@tanstack/react-query";
import { MemberRole } from "shared/types";
import type { OrganizationMemberWithProfile } from "shared/types";
import { useUserOrganizations } from "@/shared/hooks/useUserOrganizations";
import {
  useSelectedOrgId,
  useSetSelectedOrgId,
} from "@/shared/stores/useOrganizationStore";
import {
  getProjectMembers,
  listOrganizationMembers,
  listOrganizationProjects,
  listProjectApprovalSettings,
  setProjectApprovalSetting,
  setProjectMembers,
} from "@remote/shared/lib/api";

function memberName(m: OrganizationMemberWithProfile): string {
  const full = [m.first_name, m.last_name].filter(Boolean).join(" ").trim();
  return full || m.username || m.email || m.user_id;
}

export default function ProjectAssignmentsPage() {
  const { data: orgsResponse, isLoading: orgsLoading } = useUserOrganizations();
  const selectedOrgId = useSelectedOrgId();
  const setSelectedOrgId = useSetSelectedOrgId();

  const adminOrgs = useMemo(
    () =>
      (orgsResponse?.organizations ?? []).filter(
        (o) => o.user_role === MemberRole.ADMIN,
      ),
    [orgsResponse],
  );
  const activeOrg =
    adminOrgs.find((o) => o.id === selectedOrgId) ?? adminOrgs[0] ?? null;
  const orgId = activeOrg?.id ?? null;

  const projectsQuery = useQuery({
    queryKey: ["org-projects", orgId],
    queryFn: () => listOrganizationProjects(orgId as string),
    enabled: !!orgId,
  });
  const membersQuery = useQuery({
    queryKey: ["org-members", orgId],
    queryFn: () => listOrganizationMembers(orgId as string),
    enabled: !!orgId,
  });
  const approvalQuery = useQuery({
    queryKey: ["project-approval-settings", orgId],
    queryFn: () => listProjectApprovalSettings(orgId as string),
    enabled: !!orgId,
  });
  const approvalByProject = useMemo(() => {
    const map = new Map<string, boolean>();
    (approvalQuery.data ?? []).forEach((s) =>
      map.set(s.project_id, s.requires_done_approval),
    );
    return map;
  }, [approvalQuery.data]);

  const toggleApproval = useMutation({
    mutationFn: ({ projectId, value }: { projectId: string; value: boolean }) =>
      setProjectApprovalSetting(projectId, value),
    onSuccess: () => approvalQuery.refetch(),
  });

  const projects = useMemo(
    () =>
      [...(projectsQuery.data ?? [])].sort(
        (a, b) => a.sort_order - b.sort_order,
      ),
    [projectsQuery.data],
  );
  const members = membersQuery.data ?? [];

  // Current assignments per project (one query each, cached by react-query).
  const assignmentQueries = useQueries({
    queries: projects.map((p) => ({
      queryKey: ["project-members", p.id],
      queryFn: () => getProjectMembers(p.id),
      enabled: !!orgId,
    })),
  });

  if (orgsLoading) {
    return (
      <div className="mx-auto max-w-5xl px-base py-double text-sm text-low">
        Loading…
      </div>
    );
  }

  if (adminOrgs.length === 0) {
    return (
      <div className="mx-auto max-w-5xl px-base py-double">
        <Link to="/" className="text-sm text-low hover:text-high">
          ← Back
        </Link>
        <div className="mt-base rounded-sm border border-border bg-secondary p-double text-center text-sm text-low">
          Admin access required to assign projects.
        </div>
      </div>
    );
  }

  return (
    <div className="mx-auto max-w-4xl px-base py-double">
      <div className="flex flex-wrap items-center justify-between gap-base">
        <div>
          <Link to="/" className="text-xs text-low hover:text-high">
            ← Back
          </Link>
          <h1 className="mt-half text-xl font-medium text-high">
            Project assignments
          </h1>
          <p className="text-sm text-low">
            Assign projects to team members. Each member sees their assigned
            projects in the “Personal” tab of the launcher.
          </p>
        </div>

        {adminOrgs.length > 1 ? (
          <select
            value={orgId ?? ""}
            onChange={(e) => setSelectedOrgId(e.target.value)}
            className="rounded-sm border border-border bg-primary px-base py-half text-sm text-high"
          >
            {adminOrgs.map((o) => (
              <option key={o.id} value={o.id}>
                {o.name}
              </option>
            ))}
          </select>
        ) : null}
      </div>

      {projectsQuery.error || membersQuery.error ? (
        <div className="mt-base rounded-sm border border-destructive/40 bg-secondary p-base text-sm text-destructive">
          Failed to load projects or members.
        </div>
      ) : projects.length === 0 ? (
        <div className="mt-base rounded-sm border border-border bg-secondary p-double text-center text-sm text-low">
          No projects in this organization yet.
        </div>
      ) : (
        <ul className="mt-base space-y-base">
          {projects.map((project, i) => (
            <ProjectRow
              key={project.id}
              projectId={project.id}
              projectName={project.name}
              members={members}
              assigned={assignmentQueries[i]?.data ?? []}
              loading={assignmentQueries[i]?.isLoading ?? false}
              onSaved={() => assignmentQueries[i]?.refetch()}
              requiresApproval={approvalByProject.get(project.id) ?? false}
              onToggleApproval={(value) =>
                toggleApproval.mutate({ projectId: project.id, value })
              }
            />
          ))}
        </ul>
      )}
    </div>
  );
}

function ProjectRow({
  projectId,
  projectName,
  members,
  assigned,
  loading,
  onSaved,
  requiresApproval,
  onToggleApproval,
}: {
  projectId: string;
  projectName: string;
  members: OrganizationMemberWithProfile[];
  assigned: { user_id: string }[];
  loading: boolean;
  onSaved: () => void;
  requiresApproval: boolean;
  onToggleApproval: (value: boolean) => void;
}) {
  const [editing, setEditing] = useState(false);
  const assignedIds = useMemo(
    () => new Set(assigned.map((a) => a.user_id)),
    [assigned],
  );
  const [draft, setDraft] = useState<Set<string>>(new Set());

  const mutation = useMutation({
    mutationFn: (userIds: string[]) => setProjectMembers(projectId, userIds),
    onSuccess: () => {
      setEditing(false);
      onSaved();
    },
  });

  function startEditing() {
    setDraft(new Set(assignedIds));
    setEditing(true);
  }

  function toggle(userId: string) {
    setDraft((prev) => {
      const next = new Set(prev);
      if (next.has(userId)) next.delete(userId);
      else next.add(userId);
      return next;
    });
  }

  const assignedNames = members
    .filter((m) => assignedIds.has(m.user_id))
    .map(memberName);

  return (
    <li className="rounded-sm border border-border bg-primary px-base py-base">
      <div className="flex items-center justify-between gap-base">
        <div className="min-w-0">
          <p className="truncate text-sm font-medium text-high">
            {projectName}
          </p>
          <p className="mt-half text-xs text-low">
            {loading
              ? "Loading…"
              : assignedNames.length === 0
                ? "No one assigned"
                : `${assignedNames.length} assigned: ${assignedNames.join(", ")}`}
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-base">
          <label className="flex items-center gap-half text-xs text-low">
            <input
              type="checkbox"
              checked={requiresApproval}
              onChange={(e) => onToggleApproval(e.target.checked)}
            />
            Require approval
          </label>
          {!editing ? (
            <button
              type="button"
              onClick={startEditing}
              className="rounded-sm border border-border bg-secondary px-base py-half text-xs font-medium text-normal hover:border-brand/60 hover:text-high"
            >
              Manage
            </button>
          ) : null}
        </div>
      </div>

      {editing ? (
        <div className="mt-base border-t border-border pt-base">
          {members.length === 0 ? (
            <p className="text-xs text-low">No members in this organization.</p>
          ) : (
            <div className="grid gap-half sm:grid-cols-2">
              {members.map((m) => (
                <label
                  key={m.user_id}
                  className="flex items-center gap-base rounded-sm px-half py-half text-sm text-normal hover:bg-panel"
                >
                  <input
                    type="checkbox"
                    checked={draft.has(m.user_id)}
                    onChange={() => toggle(m.user_id)}
                  />
                  <span className="min-w-0 truncate">
                    {memberName(m)}
                    {m.role === MemberRole.ADMIN ? (
                      <span className="ml-half text-xs text-low">(admin)</span>
                    ) : null}
                  </span>
                </label>
              ))}
            </div>
          )}

          {mutation.error ? (
            <p className="mt-half text-xs text-destructive">
              {mutation.error instanceof Error
                ? mutation.error.message
                : "Failed to save."}
            </p>
          ) : null}

          <div className="mt-base flex items-center gap-base">
            <button
              type="button"
              disabled={mutation.isPending}
              onClick={() => mutation.mutate([...draft])}
              className="rounded-sm bg-brand px-base py-half text-xs font-medium text-on-brand hover:bg-brand-hover disabled:opacity-50"
            >
              {mutation.isPending ? "Saving…" : "Save"}
            </button>
            <button
              type="button"
              disabled={mutation.isPending}
              onClick={() => setEditing(false)}
              className="rounded-sm border border-border px-base py-half text-xs text-low hover:text-high"
            >
              Cancel
            </button>
          </div>
        </div>
      ) : null}
    </li>
  );
}
