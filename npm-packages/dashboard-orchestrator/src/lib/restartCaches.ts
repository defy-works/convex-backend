import type { ScopedMutator } from "swr";

/**
 * SWR key prefixes that a deployment restart makes stale.
 *
 * `deploymentAuth` is the load-bearing one. Restarting recreates the backend
 * container, and for a legacy row (`backend_instance_secret IS NULL`) the
 * provisioner mints a fresh `INSTANCE_SECRET`, so the backend derives a new
 * admin key and the one the dashboard already fetched stops decrypting.
 *
 * Nothing invalidated that entry, and the app sets
 * `revalidateOnFocus: false` / `revalidateOnReconnect: false` globally, so the
 * dashboard kept presenting the pre-restart key indefinitely. The backend
 * answers `BadAdminKey`, and the Convex browser client responds to an auth
 * rejection on an admin-auth session by clearing the key outright and
 * reconnecting unauthenticated — after which every `_system/*` query fails with
 * `Operation query not permitted`, and only a full page reload recovers.
 *
 * `deployments` carries each deployment's `url`/`site_url`, which a restart is
 * also the moment that changes (that is how a pending canonical URL goes live).
 */
export const RESTART_INVALIDATED_KEY_PREFIXES = [
  "deployments",
  "deploymentAuth",
] as const;

/** Matches the array-form SWR keys a restart invalidates. */
export function isRestartInvalidatedKey(key: unknown): boolean {
  return (
    Array.isArray(key) &&
    typeof key[0] === "string" &&
    (RESTART_INVALIDATED_KEY_PREFIXES as readonly string[]).includes(key[0])
  );
}

/**
 * Drop everything a restart invalidated so it refetches, rather than showing
 * the pre-restart hostname and admin key until the operator reloads.
 *
 * Call this after every `restartDeployment`.
 */
export async function invalidateAfterRestart(
  mutate: ScopedMutator,
): Promise<void> {
  await mutate(isRestartInvalidatedKey, undefined, { revalidate: true });
}
