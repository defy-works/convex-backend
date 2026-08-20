// HTTP client for the convex-orchestrator API surface. Mirrors the route
// shapes the orchestrator (`crates/orchestrator`) exposes:
//
//   POST /api/authorize                     - login
//   GET  /api/dashboard/profile             - current member
//   GET  /api/dashboard/teams               - teams
//   POST /api/dashboard/teams               - create team
//   GET  /api/dashboard/teams/{id}/projects - projects in team
//   POST /api/create_project                - create project
//   GET  /v1/projects/{id}/list_deployments - deployments
//   POST /v1/projects/{id}/create_deployment- provision deployment
//   POST /api/dashboard/instances/{name}/auth - mint deployment admin key

import { z } from "zod";

// ---------- Schemas ----------

export const memberSchema = z.object({
  id: z.number(),
  email: z.string(),
  name: z.string().nullable(),
});
export type Member = z.infer<typeof memberSchema>;

export const teamSchema = z.object({
  id: z.number(),
  name: z.string(),
  slug: z.string(),
  creator: z.number().nullable().optional(),
});
export type Team = z.infer<typeof teamSchema>;

export const projectSchema = z.object({
  id: z.number(),
  teamId: z.number(),
  name: z.string(),
  slug: z.string(),
  isDemo: z.boolean(),
  creationTime: z.number(),
});
export type Project = z.infer<typeof projectSchema>;

export const deploymentSchema = z.object({
  id: z.number(),
  projectId: z.number(),
  name: z.string(),
  kind: z.string().optional(),
  deploymentType: z.string().optional(),
  deploymentClass: z.string().optional(),
  url: z.string(),
  siteUrl: z.string(),
  state: z.string(),
  creationTime: z.number(),
  region: z.string().nullable().optional(),
  previewIdentifier: z.string().nullable().optional(),
  // Optional for backward compat with orchestrator builds that pre-date the
  // tier-on-platform-response field. Defaults to "S16" downstream when absent.
  tier: z.string().optional(),
});
export type Deployment = z.infer<typeof deploymentSchema>;

// ---------- Errors ----------

export class OrchestratorApiError extends Error {
  status: number;
  code?: string;
  constructor(status: number, message: string, code?: string) {
    super(message);
    this.status = status;
    this.code = code;
  }
}

// ---------- Internals ----------

/** Number of extra attempts for a read that died before producing a response. */
const NETWORK_RETRIES = 2;
const RETRY_DELAY_MS = 400;

/**
 * `fetch`, retrying a **read** that failed at the transport layer.
 *
 * Adding or removing a custom domain rewrites Traefik's dynamic config, and
 * Traefik reloads when it lands. Dashboard traffic travels back through that
 * same Traefik, so the reload can drop an in-flight connection — and the
 * request most likely to be in flight is the list refetch the mutation itself
 * triggers. That surfaced as "Could not load custom domains: NetworkError when
 * attempting to fetch resource" for an operation that had actually succeeded.
 *
 * Only retried when there is no response at all (`fetch` rejects, which the
 * browser does for a dropped connection) and only for GET, which is
 * idempotent. A mutation that may have already been applied is never replayed,
 * and any HTTP status — including 5xx — is returned untouched for the caller
 * to interpret.
 */
async function fetchWithRetry(
  url: string,
  init: RequestInit,
): Promise<Response> {
  const method = (init.method ?? "GET").toUpperCase();
  // Total attempts, not extra ones — so the delay below is only paid between
  // attempts that will actually happen. A mutation gets exactly one.
  const attempts = method === "GET" ? NETWORK_RETRIES + 1 : 1;
  let lastError: unknown;
  for (let attempt = 0; attempt < attempts; attempt++) {
    try {
      return await fetch(url, init);
    } catch (err) {
      lastError = err;
      if (attempt < attempts - 1) {
        await new Promise((resolve) => setTimeout(resolve, RETRY_DELAY_MS));
      }
    }
  }
  throw lastError;
}

async function request<T>(
  baseUrl: string,
  path: string,
  init: RequestInit & { auth?: boolean; token?: string | null } = {},
): Promise<T> {
  const headers: Record<string, string> = {
    "Content-Type": "application/json",
    Accept: "application/json",
    ...((init.headers as Record<string, string>) ?? {}),
  };
  const useAuth = init.auth !== false;
  if (useAuth && init.token) {
    headers.Authorization = `Bearer ${init.token}`;
  }
  const url = `${baseUrl.replace(/\/$/, "")}${path}`;
  const res = await fetchWithRetry(url, { ...init, headers });
  if (!res.ok) {
    let message = res.statusText;
    let code: string | undefined;
    try {
      const body = (await res.json()) as { code?: string; message?: string };
      message = body.message ?? message;
      code = body.code;
    } catch {
      /* ignore */
    }
    throw new OrchestratorApiError(res.status, message, code);
  }
  // Not every success carries a body: delete answers 200 and retry answers
  // 202, both empty. `res.json()` throws on an empty body, and callers await
  // this before revalidating their SWR cache — so a throw here is what leaves
  // a removed domain on screen until the operator reloads the page. Read the
  // body as text and only parse when there is something to parse.
  const text = await res.text();
  if (!text) return undefined as T;
  return JSON.parse(text) as T;
}

// ---------- Public API ----------

export type AuthorizeResponse = {
  accessToken: string;
  memberId: number;
};

export async function authorizeWithBootstrapToken(
  baseUrl: string,
  bootstrapToken: string,
  deviceName = "dashboard-orchestrator",
): Promise<AuthorizeResponse> {
  return request<AuthorizeResponse>(baseUrl, "/api/authorize", {
    method: "POST",
    auth: false,
    body: JSON.stringify({ deviceName, bootstrapToken }),
  });
}

export async function authorizeWithPassword(
  baseUrl: string,
  email: string,
  password: string,
  deviceName = "dashboard-orchestrator",
): Promise<AuthorizeResponse> {
  return request<AuthorizeResponse>(baseUrl, "/api/authorize", {
    method: "POST",
    auth: false,
    body: JSON.stringify({ deviceName, email, password }),
  });
}

export async function getProfile(
  baseUrl: string,
  token: string,
): Promise<Member> {
  return memberSchema.parse(
    await request<unknown>(baseUrl, "/api/dashboard/profile", { token }),
  );
}

export async function listTeams(
  baseUrl: string,
  token: string,
): Promise<Team[]> {
  const data = await request<unknown>(baseUrl, "/api/dashboard/teams", {
    token,
  });
  return z.array(teamSchema).parse(data);
}

export async function createTeam(
  baseUrl: string,
  token: string,
  name: string,
): Promise<Team> {
  const data = await request<unknown>(baseUrl, "/api/dashboard/teams", {
    method: "POST",
    token,
    body: JSON.stringify({ name }),
  });
  return teamSchema.parse(data);
}

export async function listProjects(
  baseUrl: string,
  token: string,
  teamId: number,
): Promise<Project[]> {
  const data = await request<unknown>(
    baseUrl,
    `/api/dashboard/teams/${teamId}/projects`,
    { token },
  );
  return z.array(projectSchema).parse(data);
}

export type CreateProjectResponse = {
  projectId: number;
  projectSlug: string;
  teamSlug: string;
  deploymentName: string | null;
  url: string | null;
  adminKey: string | null;
};

export async function createProject(
  baseUrl: string,
  token: string,
  teamSlug: string,
  projectName: string,
  deploymentType: "prod" | "dev" | null = "prod",
  tier?: string,
  knobOverrides?: Record<string, string>,
  provisioningMode?: "default" | "volume-sqlite" | "sidecar",
): Promise<CreateProjectResponse> {
  return request<CreateProjectResponse>(baseUrl, "/api/create_project", {
    method: "POST",
    token,
    body: JSON.stringify({
      team: teamSlug,
      projectName,
      deploymentType,
      tier,
      provisioningMode,
      knobOverrides,
    }),
  });
}

export async function listDeployments(
  baseUrl: string,
  token: string,
  projectId: number,
): Promise<Deployment[]> {
  const data = await request<unknown>(
    baseUrl,
    `/v1/projects/${projectId}/list_deployments`,
    { token },
  );
  return z.array(deploymentSchema).parse(data);
}

// Team-level listing — single round trip for the deployments tab on the
// team home page. Backed by GET /v1/teams/{team_id}/list_deployments.
export async function listDeploymentsForTeam(
  baseUrl: string,
  token: string,
  teamId: number,
): Promise<Deployment[]> {
  const data = await request<{ deployments: unknown[] }>(
    baseUrl,
    `/v1/teams/${teamId}/list_deployments`,
    { token },
  );
  return z.array(deploymentSchema).parse(data.deployments ?? data);
}

export async function createDeployment(
  baseUrl: string,
  token: string,
  projectId: number,
  kind: "prod" | "dev" | "preview",
): Promise<Deployment> {
  const data = await request<unknown>(
    baseUrl,
    `/v1/projects/${projectId}/create_deployment`,
    {
      method: "POST",
      token,
      body: JSON.stringify({ kind }),
    },
  );
  return deploymentSchema.parse(data);
}

export type DeploymentAuth = {
  adminKey: string;
  /** Canonical origin — what the deployment's apps use, and what we display. */
  url: string;
  /**
   * Origin to actually connect the dashboard over: the orchestrator-derived
   * hostname, never a canonical override. Optional so a dashboard newer than
   * its orchestrator still works, falling back to `url`.
   */
  consoleUrl?: string;
};

export async function fetchDeploymentAuth(
  baseUrl: string,
  token: string,
  deploymentName: string,
): Promise<DeploymentAuth> {
  return request<DeploymentAuth>(
    baseUrl,
    `/api/dashboard/instances/${deploymentName}/auth`,
    { method: "POST", token },
  );
}

// ---------- Project settings / host capacity / knob registry ----------

export const projectSettingsResponseSchema = z.object({
  tier: z.string(),
  knobOverrides: z.record(z.string(), z.string()),
});
export type ProjectSettings = z.infer<typeof projectSettingsResponseSchema>;

export async function getProjectSettings(
  baseUrl: string,
  token: string,
  projectId: number,
): Promise<ProjectSettings> {
  const data = await request<unknown>(
    baseUrl,
    `/v1/projects/${projectId}/settings`,
    { token },
  );
  return projectSettingsResponseSchema.parse(data);
}

export async function patchProjectSettings(
  baseUrl: string,
  token: string,
  projectId: number,
  patch: {
    tier?: string;
    knobOverrides?: Record<string, string | null>;
  },
): Promise<ProjectSettings> {
  const data = await request<unknown>(
    baseUrl,
    `/v1/projects/${projectId}/settings`,
    {
      method: "PATCH",
      token,
      body: JSON.stringify(patch),
    },
  );
  return projectSettingsResponseSchema.parse(data);
}

export const hostCapacityResponseSchema = z.object({
  totalMemoryMb: z.number(),
  totalCpus: z.number(),
  allocatedMemoryMb: z.number(),
  allocatedCpus: z.number(),
  deploymentCount: z.number(),
});
export type HostCapacity = z.infer<typeof hostCapacityResponseSchema>;

export async function getHostCapacity(
  baseUrl: string,
  token: string,
): Promise<HostCapacity> {
  const data = await request<unknown>(baseUrl, "/api/dashboard/host_capacity", {
    token,
  });
  return hostCapacityResponseSchema.parse(data);
}

export const knobEntrySchema = z.object({
  envVar: z.string(),
  description: z.string(),
  category: z.string(),
  exposure: z.enum(["curated", "tierTuned", "advanced"]),
  displayName: z.string().nullable(),
  defaultValue: z
    .string()
    .nullable()
    .optional()
    .transform((value) => value ?? null),
});
export type KnobEntry = z.infer<typeof knobEntrySchema>;

export async function getKnobRegistry(
  baseUrl: string,
  token: string,
): Promise<KnobEntry[]> {
  const data = await request<{ knobs: unknown[] }>(
    baseUrl,
    "/api/dashboard/knob_registry",
    { token },
  );
  return z.array(knobEntrySchema).parse(data.knobs);
}

// ---------- Deployment-level settings / restart ----------

export const deploymentSettingsResponseSchema = z.object({
  effectiveTier: z.string(),
  desiredTier: z.string().nullable(),
  desiredOverrides: z.record(z.string(), z.string()),
  runningTier: z.string(),
  runningOverrides: z.record(z.string(), z.string()),
});
export type DeploymentSettings = z.infer<
  typeof deploymentSettingsResponseSchema
>;

export async function getDeploymentSettings(
  baseUrl: string,
  token: string,
  deploymentName: string,
): Promise<DeploymentSettings> {
  const data = await request<unknown>(
    baseUrl,
    `/v1/deployments/${encodeURIComponent(deploymentName)}/settings`,
    { token },
  );
  return deploymentSettingsResponseSchema.parse(data);
}

export async function patchDeploymentSettings(
  baseUrl: string,
  token: string,
  deploymentName: string,
  patch: {
    // `undefined` = leave unchanged, `null` = clear (fall back to project
    // tier), string = set as override.
    desiredTier?: string | null;
    desiredOverrides?: Record<string, string | null>;
  },
): Promise<DeploymentSettings> {
  const body: Record<string, unknown> = {};
  if (patch.desiredTier !== undefined) body.desiredTier = patch.desiredTier;
  if (patch.desiredOverrides !== undefined)
    body.desiredOverrides = patch.desiredOverrides;
  const data = await request<unknown>(
    baseUrl,
    `/v1/deployments/${encodeURIComponent(deploymentName)}/settings`,
    {
      method: "PATCH",
      token,
      body: JSON.stringify(body),
    },
  );
  return deploymentSettingsResponseSchema.parse(data);
}

export async function restartDeployment(
  baseUrl: string,
  token: string,
  deploymentName: string,
  force?: boolean,
): Promise<void> {
  await request<unknown>(
    baseUrl,
    `/v1/deployments/${encodeURIComponent(deploymentName)}/restart`,
    {
      method: "POST",
      token,
      body: JSON.stringify(force ? { force } : {}),
    },
  );
}

// ---------- Custom domains ----------

export const customDomainSchema = z.object({
  id: z.number(),
  deploymentId: z.number(),
  domain: z.string(),
  certState: z.string(),
  createdAt: z.number(),
  kind: z.string(),
  // "acme" (we issue and renew) or "upstream" (something in front already
  // terminates TLS). Defaulted so a dashboard newer than the orchestrator
  // still parses rows written before the column existed.
  tlsMode: z.string().default("acme"),
  lastError: z.string().nullable(),
});

export type CustomDomain = z.infer<typeof customDomainSchema>;

export const listCustomDomainsSchema = z.object({
  domains: z.array(customDomainSchema),
  targetHost: z.string(),
  routingEnabled: z.boolean(),
});
export type ListCustomDomainsResponse = z.infer<typeof listCustomDomainsSchema>;

export const verifyCustomDomainSchema = z.object({
  domain: z.string(),
  certState: z.string(),
  error: z.string().nullable(),
});
export type VerifyCustomDomainResponse = z.infer<
  typeof verifyCustomDomainSchema
>;

// What a deployment advertises about itself. These become
// CONVEX_CLOUD_ORIGIN / CONVEX_SITE_ORIGIN on the backend container, i.e.
// what `CONVEX_SITE_URL` and every generated HTTP action / auth callback URL
// resolve to. They are baked in at container creation, so a change only lands
// on restart — `restartPending` says whether one is outstanding.
export const canonicalUrlsSchema = z.object({
  currentUrl: z.string(),
  currentSiteUrl: z.string(),
  desiredUrl: z.string().nullable(),
  desiredSiteUrl: z.string().nullable(),
  defaultUrl: z.string(),
  defaultSiteUrl: z.string(),
  restartPending: z.boolean(),
});
export type CanonicalUrls = z.infer<typeof canonicalUrlsSchema>;

export async function getCanonicalUrls(
  baseUrl: string,
  token: string,
  deploymentId: number,
): Promise<CanonicalUrls> {
  const data = await request<unknown>(
    baseUrl,
    `/api/dashboard/deployments/${deploymentId}/canonical_urls`,
    { token },
  );
  return canonicalUrlsSchema.parse(data);
}

/** `null` clears an override, putting the deployment back on its derived host. */
export async function setCanonicalUrls(
  baseUrl: string,
  token: string,
  deploymentId: number,
  urls: { url: string | null; siteUrl: string | null },
): Promise<CanonicalUrls> {
  const data = await request<unknown>(
    baseUrl,
    `/api/dashboard/deployments/${deploymentId}/canonical_urls/set`,
    { method: "POST", token, body: JSON.stringify(urls) },
  );
  return canonicalUrlsSchema.parse(data);
}

export async function listCustomDomains(
  baseUrl: string,
  token: string,
  deploymentId: number,
): Promise<ListCustomDomainsResponse> {
  const data = await request<unknown>(
    baseUrl,
    `/api/dashboard/deployments/${deploymentId}/custom_domains/list`,
    { token },
  );
  return listCustomDomainsSchema.parse(data);
}

export async function createCustomDomain(
  baseUrl: string,
  token: string,
  deploymentId: number,
  domain: string,
  kind: "api" | "site" = "api",
): Promise<CustomDomain> {
  const data = await request<unknown>(
    baseUrl,
    `/api/dashboard/deployments/${deploymentId}/custom_domains/create`,
    {
      method: "POST",
      token,
      body: JSON.stringify({ domain, kind }),
    },
  );
  return customDomainSchema.parse(data);
}

/** Re-runs issuance for a domain whose last attempt failed. */
export async function retryCustomDomain(
  baseUrl: string,
  token: string,
  deploymentId: number,
  domain: string,
): Promise<void> {
  await request<unknown>(
    baseUrl,
    `/api/dashboard/deployments/${deploymentId}/custom_domains/retry`,
    { method: "POST", token, body: JSON.stringify({ domain }) },
  );
}

export async function deleteCustomDomain(
  baseUrl: string,
  token: string,
  deploymentId: number,
  domain: string,
): Promise<void> {
  await request<unknown>(
    baseUrl,
    `/api/dashboard/deployments/${deploymentId}/custom_domains/delete`,
    { method: "POST", token, body: JSON.stringify({ domain }) },
  );
}

// ---------- Team invitations ----------

export const invitationSchema = z.object({
  id: z.number(),
  email: z.string(),
  role: z.string(),
  code: z.string(),
  createdAt: z.number(),
});
export type Invitation = z.infer<typeof invitationSchema>;

export async function listInvitations(
  baseUrl: string,
  token: string,
  teamId: number,
): Promise<Invitation[]> {
  const data = await request<unknown>(
    baseUrl,
    `/api/dashboard/teams/${teamId}/invites`,
    { token },
  );
  return z.array(invitationSchema).parse(data);
}

export async function createInvitation(
  baseUrl: string,
  token: string,
  teamId: number,
  email: string,
  role: string,
): Promise<void> {
  await request<unknown>(baseUrl, `/api/dashboard/teams/${teamId}/invites`, {
    method: "POST",
    token,
    body: JSON.stringify({ email, role }),
  });
}

/** Withdraw a pending invitation. Team-admin only, and scoped to the team. */
export async function cancelInvitation(
  baseUrl: string,
  token: string,
  teamId: number,
  invitationId: number,
): Promise<void> {
  await request<unknown>(
    baseUrl,
    `/api/dashboard/teams/${teamId}/invites/cancel`,
    { method: "POST", token, body: JSON.stringify({ invitationId }) },
  );
}

export async function verifyCustomDomain(
  baseUrl: string,
  token: string,
  deploymentId: number,
  domain: string,
): Promise<VerifyCustomDomainResponse> {
  const data = await request<unknown>(
    baseUrl,
    `/api/dashboard/deployments/${deploymentId}/custom_domains/verify`,
    { method: "POST", token, body: JSON.stringify({ domain }) },
  );
  return verifyCustomDomainSchema.parse(data);
}

// ---------- Personal access tokens ----------
//
// These are member-scoped: `create_personal_access_token` stores
// `member_id = <caller>` and leaves `team_id` NULL. Read them back through
// `list_personal_access_tokens`, which filters on `member_id`. The
// team-scoped `/api/dashboard/teams/{team_id}/access_tokens` filters on
// `team_id` and so can never return a personal access token.

export const personalAccessTokenSchema = z.object({
  id: z.string(),
  name: z.string(),
  creationTime: z.number(),
  keySuffix: z.string(),
  // Omitted entirely by the orchestrator when the token never expires.
  expiresAt: z.number().nullable().optional(),
});
export type PersonalAccessToken = z.infer<typeof personalAccessTokenSchema>;

const paginatedPersonalAccessTokensSchema = z.object({
  tokens: z.array(personalAccessTokenSchema),
  cursor: z.string().nullable().optional(),
});

export const createdPersonalAccessTokenSchema = z.object({
  /** Full secret. Shown once at creation and never retrievable again. */
  accessToken: z.string(),
  id: z.string(),
  name: z.string(),
  creationTime: z.number(),
});
export type CreatedPersonalAccessToken = z.infer<
  typeof createdPersonalAccessTokenSchema
>;

export async function listPersonalAccessTokens(
  baseUrl: string,
  token: string,
): Promise<PersonalAccessToken[]> {
  const data = await request<unknown>(
    baseUrl,
    "/v1/list_personal_access_tokens",
    { token },
  );
  return paginatedPersonalAccessTokensSchema.parse(data).tokens;
}

export async function createPersonalAccessToken(
  baseUrl: string,
  token: string,
  name: string,
): Promise<CreatedPersonalAccessToken> {
  const data = await request<unknown>(
    baseUrl,
    "/v1/create_personal_access_token",
    { method: "POST", token, body: JSON.stringify({ name }) },
  );
  return createdPersonalAccessTokenSchema.parse(data);
}

export async function deletePersonalAccessToken(
  baseUrl: string,
  token: string,
  id: string,
): Promise<void> {
  await request<unknown>(baseUrl, "/v1/delete_personal_access_token", {
    method: "POST",
    token,
    body: JSON.stringify({ id }),
  });
}
