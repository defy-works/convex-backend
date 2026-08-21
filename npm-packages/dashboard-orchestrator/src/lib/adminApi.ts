// Typed client for the orchestrator's instance-admin surface
// (`crates/orchestrator/src/routes/admin`).
//
// Every route here is gated server-side by the SuperAdmin extractor; these
// schemas mirror the serde-serialized response shapes. Requests go through
// `orchestratorApi`'s shared `request` helper so retry behaviour and error
// types match the rest of the dashboard.

import { z } from "zod";
import { request } from "./orchestratorApi";

export const memberTeamRefSchema = z.object({
  teamId: z.number(),
  teamSlug: z.string(),
  teamName: z.string(),
  role: z.string(),
});
export type MemberTeamRef = z.infer<typeof memberTeamRefSchema>;

export const adminMemberSchema = z.object({
  id: z.number(),
  primaryEmail: z.string(),
  name: z.string().nullable(),
  creationTime: z.number(),
  isSuperAdmin: z.boolean(),
  suspended: z.boolean(),
  teams: z.array(memberTeamRefSchema),
});
export type AdminMember = z.infer<typeof adminMemberSchema>;

/**
 * The fleet row. `AdminDeploymentRow` is flattened into the entry
 * server-side via `#[serde(flatten)]`, so these are all one object on the
 * wire rather than a nested `deployment` key.
 */
export const fleetEntrySchema = z.object({
  id: z.number(),
  name: z.string(),
  deploymentType: z.string(),
  intendedState: z.string(),
  tier: z.string(),
  url: z.string(),
  creationTime: z.number(),
  teamId: z.number(),
  teamSlug: z.string(),
  projectId: z.number(),
  projectSlug: z.string(),
  actualState: z.string(),
  drifted: z.boolean(),
});
export type FleetEntry = z.infer<typeof fleetEntrySchema>;

export const fleetResponseSchema = z.object({
  deployments: z.array(fleetEntrySchema),
  driftCount: z.number(),
  containerStatesAvailable: z.boolean(),
});
export type FleetResponse = z.infer<typeof fleetResponseSchema>;

export const overviewSchema = z.object({
  totalMemoryMb: z.number(),
  totalCpus: z.number(),
  allocatedMemoryMb: z.number(),
  allocatedCpus: z.number(),
  deploymentCount: z.number(),
  deploymentsByState: z.record(z.string(), z.number()),
  teamCount: z.number(),
  memberCount: z.number(),
});
export type Overview = z.infer<typeof overviewSchema>;

export const adminHealthSchema = z.object({
  version: z.string(),
  database: z.object({
    reachable: z.boolean(),
    pingMs: z.number().nullable(),
    error: z.string().nullable(),
  }),
  provisioner: z.object({
    mode: z.string(),
    // null when the provisioner does not manage containers, which is not
    // the same as "the socket is down".
    dockerReachable: z.boolean().nullable(),
    error: z.string().nullable(),
  }),
  reconcileIntervalSecs: z.number(),
});
export type AdminHealth = z.infer<typeof adminHealthSchema>;

export const instanceAuditEventSchema = z.object({
  id: z.number(),
  memberId: z.number().nullable(),
  action: z.string(),
  metadata: z.unknown(),
  creationTime: z.number(),
});
export type InstanceAuditEvent = z.infer<typeof instanceAuditEventSchema>;

async function adminGet<T>(
  baseUrl: string,
  path: string,
  token: string,
  schema: z.ZodType<T>,
): Promise<T> {
  const data = await request<unknown>(baseUrl, path, { token });
  return schema.parse(data);
}

export function getAdminOverview(baseUrl: string, token: string) {
  return adminGet(baseUrl, "/api/admin/overview", token, overviewSchema);
}

export function getAdminHealth(baseUrl: string, token: string) {
  return adminGet(baseUrl, "/api/admin/health", token, adminHealthSchema);
}

export function getAdminFleet(baseUrl: string, token: string) {
  return adminGet(baseUrl, "/api/admin/fleet", token, fleetResponseSchema);
}

export function getAdminMembers(baseUrl: string, token: string) {
  return adminGet(
    baseUrl,
    "/api/admin/members",
    token,
    z.object({ members: z.array(adminMemberSchema) }),
  );
}

export function getAdminAudit(baseUrl: string, token: string, limit = 100) {
  return adminGet(
    baseUrl,
    `/api/admin/audit?limit=${limit}`,
    token,
    z.object({ events: z.array(instanceAuditEventSchema) }),
  );
}
