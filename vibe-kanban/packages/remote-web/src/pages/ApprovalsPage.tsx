import { useMemo, useState } from "react";
import { Link } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { MemberRole } from "shared/types";
import { useUserOrganizations } from "@/shared/hooks/useUserOrganizations";
import {
  useSelectedOrgId,
  useSetSelectedOrgId,
} from "@/shared/stores/useOrganizationStore";
import {
  approveIssue,
  listPendingApprovals,
  rejectIssue,
  type PendingApproval,
} from "@remote/shared/lib/api";

function timeAgo(iso: string): string {
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return "";
  const mins = Math.round((Date.now() - then) / 60000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.round(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.round(hours / 24)}d ago`;
}

export default function ApprovalsPage() {
  const { data: orgsResponse, isLoading: orgsLoading } = useUserOrganizations();
  const selectedOrgId = useSelectedOrgId();
  const setSelectedOrgId = useSetSelectedOrgId();
  const queryClient = useQueryClient();

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

  const { data: pending, isLoading } = useQuery({
    queryKey: ["pending-approvals", orgId],
    queryFn: () => listPendingApprovals(orgId as string),
    enabled: !!orgId,
  });

  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: ["pending-approvals", orgId] });

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
          Admin access required to review approvals.
        </div>
      </div>
    );
  }

  return (
    <div className="mx-auto max-w-3xl px-base py-double">
      <div className="flex flex-wrap items-center justify-between gap-base">
        <div>
          <Link to="/" className="text-xs text-low hover:text-high">
            ← Back
          </Link>
          <h1 className="mt-half text-xl font-medium text-high">
            Pending approvals
          </h1>
          <p className="text-sm text-low">
            Issues developers marked Done in approval-required projects, awaiting
            your sign-off.
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

      {isLoading ? (
        <p className="mt-base text-sm text-low">Loading…</p>
      ) : !pending || pending.length === 0 ? (
        <div className="mt-base rounded-sm border border-border bg-secondary p-double text-center text-sm text-low">
          Nothing awaiting approval. 🎉
        </div>
      ) : (
        <ul className="mt-base space-y-base">
          {pending.map((item) => (
            <ApprovalRow key={item.issue_id} item={item} onDone={invalidate} />
          ))}
        </ul>
      )}
    </div>
  );
}

function ApprovalRow({
  item,
  onDone,
}: {
  item: PendingApproval;
  onDone: () => void;
}) {
  const [rejecting, setRejecting] = useState(false);
  const [note, setNote] = useState("");

  const approve = useMutation({
    mutationFn: () => approveIssue(item.issue_id),
    onSuccess: onDone,
  });
  const reject = useMutation({
    mutationFn: () => rejectIssue(item.issue_id, note),
    onSuccess: onDone,
  });
  const busy = approve.isPending || reject.isPending;

  return (
    <li className="rounded-sm border border-border bg-primary px-base py-base">
      <div className="flex items-start justify-between gap-base">
        <div className="min-w-0">
          <p className="truncate text-sm font-medium text-high">
            <span className="text-low">{item.simple_id}</span> {item.title}
          </p>
          <p className="mt-half text-xs text-low">
            {item.project_name}
            {item.assignees ? ` • ${item.assignees}` : ""} •{" "}
            {timeAgo(item.submitted_at)}
          </p>
        </div>
        {!rejecting ? (
          <div className="flex shrink-0 items-center gap-base">
            <button
              type="button"
              disabled={busy}
              onClick={() => approve.mutate()}
              className="rounded-sm bg-brand px-base py-half text-xs font-medium text-on-brand hover:bg-brand-hover disabled:opacity-50"
            >
              {approve.isPending ? "Approving…" : "Approve"}
            </button>
            <button
              type="button"
              disabled={busy}
              onClick={() => setRejecting(true)}
              className="rounded-sm border border-border px-base py-half text-xs text-normal hover:border-destructive/60 hover:text-destructive"
            >
              Reject
            </button>
          </div>
        ) : null}
      </div>

      {rejecting ? (
        <div className="mt-base border-t border-border pt-base">
          <textarea
            value={note}
            onChange={(e) => setNote(e.target.value)}
            placeholder="Reason (sent back with the issue)…"
            rows={2}
            className="w-full rounded-sm border border-border bg-secondary px-base py-half text-sm text-high"
          />
          <div className="mt-half flex items-center gap-base">
            <button
              type="button"
              disabled={reject.isPending}
              onClick={() => reject.mutate()}
              className="rounded-sm bg-destructive px-base py-half text-xs font-medium text-white hover:opacity-90 disabled:opacity-50"
            >
              {reject.isPending ? "Rejecting…" : "Reject & return to In Progress"}
            </button>
            <button
              type="button"
              disabled={reject.isPending}
              onClick={() => setRejecting(false)}
              className="rounded-sm border border-border px-base py-half text-xs text-low hover:text-high"
            >
              Cancel
            </button>
          </div>
        </div>
      ) : null}

      {approve.error || reject.error ? (
        <p className="mt-half text-xs text-destructive">
          {(approve.error ?? reject.error) instanceof Error
            ? (approve.error ?? reject.error)!.message
            : "Action failed."}
        </p>
      ) : null}
    </li>
  );
}
