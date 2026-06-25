import { useEffect, useMemo, useState } from "react";
import { Link } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { MemberRole } from "shared/types";
import { DataTable, type ColumnDef } from "@vibe/ui/components/DataTable";
import { Badge } from "@vibe/ui/components/Badge";
import { useUserOrganizations } from "@/shared/hooks/useUserOrganizations";
import {
  useSelectedOrgId,
  useSetSelectedOrgId,
} from "@/shared/stores/useOrganizationStore";
import {
  getOrganizationInsights,
  listOrganizationProjects,
  type DeliverySummary,
  type DeveloperInsights,
  type InsightsWindow,
} from "@remote/shared/lib/api";

const WINDOWS: { value: InsightsWindow; label: string }[] = [
  { value: "7d", label: "7 days" },
  { value: "30d", label: "30 days" },
  { value: "all", label: "All time" },
];

// Numeric, sortable metric columns.
type SortKey =
  | "score"
  | "issues_assigned"
  | "issues_completed"
  | "mrs_opened"
  | "mrs_merged"
  | "last_active_at";

type Row = DeveloperInsights & { rank: number };

function formatDuration(hours: number | null): string {
  if (hours == null) return "—";
  if (hours < 1) return `${Math.round(hours * 60)}m`;
  if (hours < 24) return `${hours.toFixed(1)}h`;
  const days = hours / 24;
  return `${days.toFixed(1)}d`;
}

function formatWeek(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  return d.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

function formatLastActive(iso: string | null): string {
  if (!iso) return "—";
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return "—";
  const mins = Math.round((Date.now() - then) / 60000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.round(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.round(hours / 24);
  if (days < 30) return `${days}d ago`;
  return new Date(iso).toLocaleDateString();
}

function sortValue(dev: DeveloperInsights, key: SortKey): number {
  if (key === "last_active_at") {
    return dev.last_active_at ? new Date(dev.last_active_at).getTime() : 0;
  }
  return dev[key];
}

export default function InsightsPage() {
  const { data: orgsResponse, isLoading: orgsLoading } = useUserOrganizations();
  const selectedOrgId = useSelectedOrgId();
  const setSelectedOrgId = useSetSelectedOrgId();

  const [window, setWindow] = useState<InsightsWindow>("30d");
  // null = all projects in the org.
  const [projectId, setProjectId] = useState<string | null>(null);
  const [sortKey, setSortKey] = useState<SortKey>("score");
  // score is most useful descending; the user can flip per column.
  const [sortDir, setSortDir] = useState<"asc" | "desc">("desc");

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

  // Reset the project filter whenever the active org changes — its projects differ.
  useEffect(() => {
    setProjectId(null);
  }, [orgId]);

  const { data: projects } = useQuery({
    queryKey: ["org-projects", orgId],
    queryFn: () => listOrganizationProjects(orgId as string),
    enabled: !!orgId,
  });

  const {
    data,
    isLoading: insightsLoading,
    error,
  } = useQuery({
    queryKey: ["organization-insights", orgId, window, projectId],
    queryFn: () => getOrganizationInsights(orgId as string, window, projectId),
    enabled: !!orgId,
  });

  const rows = useMemo<Row[]>(() => {
    const developers = data?.developers ?? [];
    const sorted = [...developers].sort((a, b) => {
      const diff = sortValue(a, sortKey) - sortValue(b, sortKey);
      return sortDir === "asc" ? diff : -diff;
    });
    return sorted.map((dev, index) => ({ ...dev, rank: index + 1 }));
  }, [data, sortKey, sortDir]);

  function toggleSort(key: SortKey) {
    if (key === sortKey) {
      setSortDir((dir) => (dir === "asc" ? "desc" : "asc"));
    } else {
      setSortKey(key);
      setSortDir("desc");
    }
  }

  function sortableHeader(label: string, key: SortKey) {
    const active = sortKey === key;
    return (
      <button
        type="button"
        onClick={() => toggleSort(key)}
        className={`inline-flex items-center gap-1 transition-colors hover:text-high ${
          active ? "text-high" : ""
        }`}
      >
        {label}
        <span className="text-xs">
          {active ? (sortDir === "asc" ? "▲" : "▼") : ""}
        </span>
      </button>
    );
  }

  const numberCell = (value: number) => (
    <span className="tabular-nums">{value.toLocaleString()}</span>
  );

  const columns: ColumnDef<Row>[] = [
    {
      id: "rank",
      header: "#",
      accessor: (row) => <span className="text-low tabular-nums">{row.rank}</span>,
      className: "w-10",
    },
    {
      id: "developer",
      header: "Developer",
      accessor: (row) => (
        <div className="flex flex-col">
          <span className="font-medium text-high">{row.display_name}</span>
          <span className="text-xs text-low">{row.email}</span>
        </div>
      ),
    },
    {
      id: "role",
      header: "Role",
      accessor: (row) => (
        <Badge variant={row.role === "admin" ? "default" : "secondary"}>
          {row.role}
        </Badge>
      ),
    },
    {
      id: "last_active_at",
      header: sortableHeader("Last active", "last_active_at"),
      accessor: (row) => (
        <span className="text-low">{formatLastActive(row.last_active_at)}</span>
      ),
    },
    {
      id: "issues_assigned",
      header: sortableHeader("Assigned", "issues_assigned"),
      accessor: (row) => numberCell(row.issues_assigned),
    },
    {
      id: "issues_completed",
      header: sortableHeader("Issues done", "issues_completed"),
      accessor: (row) => numberCell(row.issues_completed),
    },
    {
      id: "mrs_opened",
      header: sortableHeader("MRs opened", "mrs_opened"),
      accessor: (row) => numberCell(row.mrs_opened),
    },
    {
      id: "mrs_merged",
      header: sortableHeader("MRs merged", "mrs_merged"),
      accessor: (row) => numberCell(row.mrs_merged),
    },
    {
      id: "score",
      header: sortableHeader("Score", "score"),
      accessor: (row) => (
        <span className="font-medium text-high tabular-nums">
          {row.score.toLocaleString()}
        </span>
      ),
    },
  ];

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
          Admin access required to view team insights.
        </div>
      </div>
    );
  }

  return (
    <div className="mx-auto max-w-6xl px-base py-double">
      <div className="flex flex-wrap items-center justify-between gap-base">
        <div>
          <Link to="/" className="text-xs text-low hover:text-high">
            ← Back
          </Link>
          <h1 className="mt-half text-xl font-medium text-high">
            Team insights
          </h1>
          <p className="text-sm text-low">
            {projectId
              ? `${
                  projects?.find((p) => p.id === projectId)?.name ?? "Project"
                } · engagement and delivery, ranked by activity score.`
              : "Engagement and delivery leaderboard, ranked by activity score."}
          </p>
        </div>

        <div className="flex items-center gap-base">
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

          {projects && projects.length > 0 ? (
            <select
              value={projectId ?? ""}
              onChange={(e) => setProjectId(e.target.value || null)}
              className="max-w-48 rounded-sm border border-border bg-primary px-base py-half text-sm text-high"
            >
              <option value="">All projects</option>
              {[...projects]
                .sort((a, b) => a.name.localeCompare(b.name))
                .map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.name}
                  </option>
                ))}
            </select>
          ) : null}

          <div className="flex rounded-sm border border-border">
            {WINDOWS.map((w) => (
              <button
                key={w.value}
                type="button"
                onClick={() => setWindow(w.value)}
                className={`px-base py-half text-xs transition-colors ${
                  window === w.value
                    ? "bg-brand text-white"
                    : "text-low hover:text-high"
                }`}
              >
                {w.label}
              </button>
            ))}
          </div>
        </div>
      </div>

      {!error && data?.summary ? (
        <DeliverySection summary={data.summary} loading={insightsLoading} />
      ) : null}

      {error ? (
        <div className="mt-base rounded-sm border border-destructive/40 bg-secondary p-base text-sm text-destructive">
          {error instanceof Error ? error.message : "Failed to load insights."}
        </div>
      ) : (
        <div className="mt-base">
          <DataTable<Row>
            data={rows}
            columns={columns}
            keyExtractor={(row) => row.user_id}
            isLoading={insightsLoading}
            emptyState="No activity in this window yet."
          />
        </div>
      )}

      <p className="mt-base text-xs text-low">
        Score = merged MRs ×5 + opened MRs ×2 + issues done ×3 + assigned issues
        ×1. Issues done = issues in a “Done” status assigned to the developer;
        MRs are attributed via the developer's workspace.
      </p>
    </div>
  );
}

function StatCard({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-sm border border-border bg-secondary px-base py-base">
      <p className="text-xs text-low">{label}</p>
      <p className="mt-half text-2xl font-semibold text-high tabular-nums">
        {value}
      </p>
    </div>
  );
}

function DeliverySection({
  summary,
  loading,
}: {
  summary: DeliverySummary;
  loading: boolean;
}) {
  const maxCount = Math.max(1, ...summary.throughput.map((b) => b.count));
  return (
    <div className={`mt-base ${loading ? "opacity-60" : ""}`}>
      <div className="grid gap-base sm:grid-cols-3">
        <StatCard
          label="Completed (in window)"
          value={summary.completed_count.toLocaleString()}
        />
        <StatCard
          label="Avg cycle time"
          value={formatDuration(summary.avg_cycle_time_hours)}
        />
        <StatCard
          label="Median cycle time"
          value={formatDuration(summary.median_cycle_time_hours)}
        />
      </div>

      <div className="mt-base rounded-sm border border-border bg-secondary p-base">
        <p className="text-xs text-low">Throughput (issues completed / week)</p>
        {summary.throughput.length === 0 ? (
          <p className="mt-base text-sm text-low">
            No issues completed in this window yet.
          </p>
        ) : (
          <div className="mt-base flex h-32 items-end gap-1 overflow-x-auto">
            {summary.throughput.map((b) => (
              <div
                key={b.week_start}
                className="flex min-w-6 flex-1 flex-col items-center justify-end gap-1"
                title={`Week of ${formatWeek(b.week_start)}: ${b.count}`}
              >
                <span className="text-xs tabular-nums text-normal">
                  {b.count}
                </span>
                <div
                  className="w-full rounded-t-sm bg-brand"
                  style={{
                    height: `${Math.max(4, (b.count / maxCount) * 96)}px`,
                  }}
                />
                <span className="text-[10px] text-low">
                  {formatWeek(b.week_start)}
                </span>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
