import { createFileRoute } from "@tanstack/react-router";
import { requireAuthenticated } from "@remote/shared/lib/route-auth";
import InsightsPage from "../pages/InsightsPage";

// Admin-only is enforced server-side (403 for non-admins) and again in the
// page, which renders an "Admin access required" state for non-admin users.
export const Route = createFileRoute("/admin/insights")({
  beforeLoad: async ({ location }) => {
    await requireAuthenticated(location);
  },
  component: InsightsPage,
});
