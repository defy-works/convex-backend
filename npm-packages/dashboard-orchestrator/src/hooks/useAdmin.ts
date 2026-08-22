// SWR hooks over the instance-admin API.
//
// All keyed on the access token so a sign-out or session swap refetches
// rather than serving the previous session's data from cache, matching
// `useHostCapacity` and the other orchestrator hooks.

import useSWR from "swr";
import {
  getAdminAudit,
  getAdminTeams,
  getAdminFleet,
  getAdminHealth,
  getAdminMembers,
  getAdminOverview,
} from "../lib/adminApi";
import { orchestratorUrl } from "../lib/config";
import { useAccessToken } from "../lib/useOrchestratorToken";

/**
 * Fleet state changes without the user doing anything — containers stop,
 * the reconciler starts them — so this one polls. The others are static
 * enough to leave on SWR's default revalidation.
 */
const FLEET_REFRESH_MS = 15_000;

export function useAdminOverview() {
  const token = useAccessToken();
  const url = orchestratorUrl();
  return useSWR(token ? ["admin/overview", token] : null, () =>
    getAdminOverview(url, token!),
  );
}

export function useAdminHealth() {
  const token = useAccessToken();
  const url = orchestratorUrl();
  return useSWR(token ? ["admin/health", token] : null, () =>
    getAdminHealth(url, token!),
  );
}

export function useAdminFleet() {
  const token = useAccessToken();
  const url = orchestratorUrl();
  return useSWR(
    token ? ["admin/fleet", token] : null,
    () => getAdminFleet(url, token!),
    { refreshInterval: FLEET_REFRESH_MS },
  );
}

export function useAdminMembers() {
  const token = useAccessToken();
  const url = orchestratorUrl();
  return useSWR(token ? ["admin/members", token] : null, () =>
    getAdminMembers(url, token!),
  );
}

export function useAdminAudit(limit = 100) {
  const token = useAccessToken();
  const url = orchestratorUrl();
  return useSWR(token ? ["admin/audit", token, limit] : null, () =>
    getAdminAudit(url, token!, limit),
  );
}

export function useAdminTeams() {
  const token = useAccessToken();
  const url = orchestratorUrl();
  return useSWR(token ? ["admin/teams", token] : null, () =>
    getAdminTeams(url, token!),
  );
}
