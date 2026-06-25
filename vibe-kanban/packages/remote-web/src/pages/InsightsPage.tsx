import { useMemo, useState } from "react";
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

  const {
    data,
    isLoading: insightsLoading,
    error,
  } = useQuery({
    queryKey: ["organization-insights", orgId, window],
    queryFn: () => getOrganizationInsights(orgId as string, window),
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
            Engagement and delivery leaderboard, ranked by activity score.
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
