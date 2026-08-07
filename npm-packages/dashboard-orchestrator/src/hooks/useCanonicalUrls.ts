import useSWR from "swr";
import { getCanonicalUrls, setCanonicalUrls } from "../lib/orchestratorApi";
import { useAccessToken } from "../lib/useOrchestratorToken";
import { orchestratorUrl } from "../lib/config";

/**
 * The origins a deployment advertises — CONVEX_CLOUD_ORIGIN and
 * CONVEX_SITE_ORIGIN on its backend container.
 *
 * Saving a change does not apply it: those are baked into the container's
 * environment, so the deployment has to be restarted before the backend
 * reports the new URL. `restartPending` is how the UI knows to say so.
 */
export function useCanonicalUrls(deploymentId: number | undefined) {
  const token = useAccessToken();
  const url = orchestratorUrl();
  const { data, error, isLoading, mutate } = useSWR(
    token && deploymentId ? ["canonicalUrls", deploymentId, token] : null,
    () => getCanonicalUrls(url, token!, deploymentId!),
  );

  const save = async (urls: { url: string | null; siteUrl: string | null }) => {
    if (!token || !deploymentId) return;
    const next = await setCanonicalUrls(url, token, deploymentId, urls);
    // Feed the server's answer straight back in — it already recomputed
    // restartPending, so a revalidation round trip would only repeat it.
    await mutate(next, { revalidate: false });
  };

  return { canonical: data, error, isLoading, save };
}
