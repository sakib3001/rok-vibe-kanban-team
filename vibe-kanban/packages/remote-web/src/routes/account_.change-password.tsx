import { createFileRoute } from "@tanstack/react-router";
import { z } from "zod";
import { zodValidator } from "@tanstack/zod-adapter";
import { requireAuthenticated } from "@remote/shared/lib/route-auth";
import ChangePasswordPage from "../pages/ChangePasswordPage";

const searchSchema = z.object({
  must: z.coerce.string().optional(),
  next: z.string().optional(),
});

// Cast required because routeTree.gen.ts is regenerated AFTER tsc runs.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export const Route = createFileRoute("/account_/change-password" as any)({
  validateSearch: zodValidator(searchSchema),
  beforeLoad: async ({ location }: { location: Parameters<typeof requireAuthenticated>[0] }) => {
    await requireAuthenticated(location);
  },
  component: ChangePasswordPage,
});
