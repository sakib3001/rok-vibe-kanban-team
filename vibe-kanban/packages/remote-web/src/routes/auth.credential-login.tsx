import { createFileRoute, redirect } from "@tanstack/react-router";

// Backend (auth/credential.rs) emits {base}/auth/credential-login as the
// invite login URL. Forward to the real login page.
// Cast is required because routeTree.gen.ts is generated AFTER tsc runs;
// the path is valid at vite-build time.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export const Route = createFileRoute("/auth/credential-login" as any)({
  beforeLoad: ({ search }: { search: Record<string, string | undefined> }) => {
    const next = search.next;
    throw redirect({
      to: "/account",
      search: next ? { next } : undefined,
    });
  },
});
