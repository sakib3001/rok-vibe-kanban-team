import { createFileRoute } from "@tanstack/react-router";
import { requireAuthenticated } from "@remote/shared/lib/route-auth";
import ApprovalsPage from "../pages/ApprovalsPage";

// Admin-only is enforced server-side (403 for non-admins) and again in the page.
export const Route = createFileRoute("/admin/approvals")({
  beforeLoad: async ({ location }) => {
    await requireAuthenticated(location);
  },
  component: ApprovalsPage,
});
