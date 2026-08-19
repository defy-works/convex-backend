// A restart must invalidate the cached admin key, not just the deployment list.
//
// Restarting recreates the backend container, and on a legacy row
// (`backend_instance_secret IS NULL`) the provisioner mints a fresh
// `INSTANCE_SECRET`, so the backend derives a new admin key and the cached one
// stops working. The dashboard then sends a key the backend rejects
// (`BadAdminKey`), and the Convex browser client reacts by clearing admin auth
// and reconnecting unauthenticated — at which point every `_system/*` query
// comes back as `Operation query not permitted` until a full page reload.

import {
  invalidateAfterRestart,
  isRestartInvalidatedKey,
} from "./restartCaches";

test("the cached admin key is invalidated", () => {
  // The key OrchestratorDeploymentShell caches the minted admin key under.
  expect(
    isRestartInvalidatedKey(["deploymentAuth", "happy-otter-123", "pat_test"]),
  ).toBe(true);
});

test("the deployment list is invalidated, since a restart changes its URLs", () => {
  expect(isRestartInvalidatedKey(["deployments", 7, "pat_test"])).toBe(true);
});

test("unrelated caches are left alone", () => {
  for (const key of [
    ["teams", "pat_test"],
    ["projects", 1, "pat_test"],
    ["personalAccessTokens", "pat_test"],
    ["hostCapacity", "pat_test"],
  ]) {
    expect(isRestartInvalidatedKey(key)).toBe(false);
  }
});

test("non-array keys are ignored rather than throwing", () => {
  for (const key of [undefined, null, "deployments", 7, {}]) {
    expect(isRestartInvalidatedKey(key)).toBe(false);
  }
});

test("invalidateAfterRestart asks SWR to revalidate the matching keys", async () => {
  const mutate = jest.fn().mockResolvedValue(undefined);
  await invalidateAfterRestart(mutate as never);

  expect(mutate).toHaveBeenCalledTimes(1);
  const [filter, data, opts] = mutate.mock.calls[0];
  expect(typeof filter).toBe("function");
  expect(data).toBeUndefined();
  expect(opts).toEqual({ revalidate: true });
  // The filter SWR receives has to be the real predicate.
  expect(filter(["deploymentAuth", "d", "t"])).toBe(true);
  expect(filter(["teams", "t"])).toBe(false);
});
