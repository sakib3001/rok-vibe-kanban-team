import { getToken, triggerRefresh } from "@remote/shared/lib/auth/tokenManager";
import { clearTokens } from "@remote/shared/lib/auth";
import type { Project } from "shared/remote-types";
import type {
  ListOrganizationsResponse,
  OrganizationMemberWithProfile,
} from "shared/types";

const API_BASE = import.meta.env.VITE_API_BASE_URL || "";

export type OAuthProvider = "github" | "google";

export type AuthMethodsResponse = {
  local_auth_enabled: boolean;
  credential_auth_enabled?: boolean;
  oauth_providers: string[];
};

type HandoffInitResponse = {
  handoff_id: string;
  authorize_url: string;
};

type HandoffRedeemResponse = {
  access_token: string;
  refresh_token: string;
};

type LocalLoginResponse = {
  access_token: string;
  refresh_token: string;
};

export type InvitationLookupResponse = {
  id: string;
  organization_slug: string;
  organization_name?: string;
  role: string;
  expires_at: string;
};

type AcceptInvitationResponse = {
  organization_id: string;
  organization_slug: string;
  role: string;
};

type IdentityResponse = {
  user_id: string;
  username: string | null;
  email: string;
};

export async function initOAuth(
  provider: OAuthProvider,
  returnTo: string,
  appChallenge: string,
): Promise<HandoffInitResponse> {
  const res = await fetch(`${API_BASE}/v1/oauth/web/init`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      provider,
      return_to: returnTo,
      app_challenge: appChallenge,
    }),
  });
  if (!res.ok) {
    throw new Error(`OAuth init failed (${res.status})`);
  }
  return res.json();
}

export async function getAuthMethods(): Promise<AuthMethodsResponse> {
  const res = await fetch(`${API_BASE}/v1/auth/methods`, {
    cache: "no-store",
  });
  if (!res.ok) {
    throw new Error(`Auth methods lookup failed (${res.status})`);
  }
  return res.json();
}

export async function redeemOAuth(
  handoffId: string,
  appCode: string,
  appVerifier: string,
): Promise<HandoffRedeemResponse> {
  const res = await fetch(`${API_BASE}/v1/oauth/web/redeem`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      handoff_id: handoffId,
      app_code: appCode,
      app_verifier: appVerifier,
    }),
  });
  if (!res.ok) {
    throw new Error(`OAuth redeem failed (${res.status})`);
  }
  return res.json();
}

export async function localLogin(
  email: string,
  password: string,
): Promise<LocalLoginResponse> {
  const res = await fetch(`${API_BASE}/v1/auth/local/login`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ email, password }),
  });
  if (!res.ok) {
    throw new Error(`Local login failed (${res.status})`);
  }
  return res.json();
}

export async function getInvitation(
  token: string,
): Promise<InvitationLookupResponse> {
  const res = await fetch(`${API_BASE}/v1/invitations/${token}`);
  if (!res.ok) {
    throw new Error(`Invitation not found (${res.status})`);
  }
  return res.json();
}

export async function acceptInvitation(
  token: string,
  accessToken: string,
): Promise<AcceptInvitationResponse> {
  const res = await fetch(`${API_BASE}/v1/invitations/${token}/accept`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${accessToken}`,
    },
  });
  if (!res.ok) {
    throw new Error(`Failed to accept invitation (${res.status})`);
  }
  return res.json();
}

export async function refreshTokens(
  refreshToken: string,
): Promise<{ access_token: string; refresh_token: string }> {
  const res = await fetch(`${API_BASE}/v1/tokens/refresh`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ refresh_token: refreshToken }),
  });
  if (!res.ok) {
    const err = new Error(`Token refresh failed (${res.status})`);
    (err as Error & { status: number }).status = res.status;
    throw err;
  }
  return res.json();
}

export async function authenticatedFetch(
  url: string,
  options: RequestInit = {},
): Promise<Response> {
  const accessToken = await getToken();

  const res = await fetch(url, {
    ...options,
    headers: {
      ...options.headers,
      Authorization: `Bearer ${accessToken}`,
    },
  });

  if (res.status === 401) {
    const newAccessToken = await triggerRefresh();
    return fetch(url, {
      ...options,
      headers: {
        ...options.headers,
        Authorization: `Bearer ${newAccessToken}`,
      },
    });
  }

  return res;
}

export async function logout(): Promise<void> {
  try {
    await authenticatedFetch(`${API_BASE}/v1/oauth/logout`, {
      method: "POST",
    });
  } finally {
    await clearTokens();
  }
}

export async function listOrganizations(): Promise<ListOrganizationsResponse> {
  const res = await authenticatedFetch(`${API_BASE}/v1/organizations`);
  if (!res.ok) {
    throw new Error(`Failed to list organizations (${res.status})`);
  }
  return res.json();
}

// --- PM analytics (insights) ---
// Phase 1 returns plain JSON from the remote server. The shape mirrors
// `InsightsResponse` in crates/remote/src/routes/insights.rs. When that struct
// gains a `TS` derive, replace these hand-written types with the generated ones.

export type InsightsWindow = "7d" | "30d" | "all";

export type DeveloperInsights = {
  user_id: string;
  email: string;
  display_name: string;
  username: string | null;
  role: string;
  last_active_at: string | null;
  issues_assigned: number;
  issues_completed: number;
  mrs_opened: number;
  mrs_merged: number;
  score: number;
};

export type ThroughputBucket = {
  week_start: string;
  count: number;
};

export type DeliverySummary = {
  completed_count: number;
  avg_cycle_time_hours: number | null;
  median_cycle_time_hours: number | null;
  throughput: ThroughputBucket[];
};

export type OrganizationInsightsResponse = {
  organization_id: string;
  window: string;
  since: string | null;
  generated_at: string;
  developers: DeveloperInsights[];
  summary: DeliverySummary;
};

export async function getOrganizationInsights(
  organizationId: string,
  window: InsightsWindow,
): Promise<OrganizationInsightsResponse> {
  const params = new URLSearchParams({ window });
  const res = await authenticatedFetch(
    `${API_BASE}/v1/organizations/${organizationId}/insights?${params}`,
  );
  if (!res.ok) {
    throw new Error(`Failed to load insights (${res.status})`);
  }
  return res.json();
}

export async function getIdentity(): Promise<IdentityResponse> {
  const res = await authenticatedFetch(`${API_BASE}/v1/identity`);
  if (!res.ok) {
    throw new Error(`Failed to fetch identity (${res.status})`);
  }
  return res.json();
}

export async function listOrganizationProjects(
  organizationId: string,
): Promise<Project[]> {
  const params = new URLSearchParams({
    organization_id: organizationId,
  });

  const res = await authenticatedFetch(`${API_BASE}/v1/projects?${params}`);
  if (!res.ok) {
    throw new Error(`Failed to list projects (${res.status})`);
  }

  const body = (await res.json()) as { projects: Project[] };
  return body.projects;
}

// --- Project assignment ---
// Admin assigns whole projects to members; the launcher's Personal tab reads
// the result. Types mirror api_types::ProjectMember / SetProjectMembersRequest.

export type ProjectMember = {
  id: string;
  project_id: string;
  user_id: string;
  assigned_at: string;
};

export async function listOrganizationMembers(
  organizationId: string,
): Promise<OrganizationMemberWithProfile[]> {
  const res = await authenticatedFetch(
    `${API_BASE}/v1/organizations/${organizationId}/members`,
  );
  if (!res.ok) {
    throw new Error(`Failed to list members (${res.status})`);
  }
  const body = (await res.json()) as {
    members: OrganizationMemberWithProfile[];
  };
  return body.members;
}

export async function getProjectMembers(
  projectId: string,
): Promise<ProjectMember[]> {
  const res = await authenticatedFetch(
    `${API_BASE}/v1/projects/${projectId}/members`,
  );
  if (!res.ok) {
    throw new Error(`Failed to load project members (${res.status})`);
  }
  const body = (await res.json()) as { project_members: ProjectMember[] };
  return body.project_members;
}

export async function setProjectMembers(
  projectId: string,
  userIds: string[],
): Promise<ProjectMember[]> {
  const res = await authenticatedFetch(
    `${API_BASE}/v1/projects/${projectId}/members`,
    {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ user_ids: userIds }),
    },
  );
  if (!res.ok) {
    throw new Error(`Failed to update project members (${res.status})`);
  }
  const body = (await res.json()) as { project_members: ProjectMember[] };
  return body.project_members;
}

export type CredentialLoginResponse = {
  access_token: string;
  refresh_token: string;
  must_change_password: boolean;
};

export async function credentialLogin(
  email: string,
  password: string,
): Promise<CredentialLoginResponse> {
  const res = await fetch(`${API_BASE}/v1/auth/credential/login`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ email, password }),
  });
  if (!res.ok) {
    throw new Error(`Credential login failed (${res.status})`);
  }
  return res.json();
}

export async function changePassword(
  currentPassword: string,
  newPassword: string,
  accessToken: string,
): Promise<void> {
  const res = await fetch(`${API_BASE}/v1/auth/credential/change-password`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${accessToken}`,
    },
    body: JSON.stringify({
      current_password: currentPassword,
      new_password: newPassword,
    }),
  });
  if (!res.ok) {
    throw new Error(`Change password failed (${res.status})`);
  }
}

export async function requestPasswordReset(email: string): Promise<void> {
  await fetch(`${API_BASE}/v1/auth/credential/reset-request`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ email }),
  });
}
