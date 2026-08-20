import useSWR from "swr";
import {
  createCustomDomain,
  deleteCustomDomain,
  listCustomDomains,
  retryCustomDomain,
  setCustomDomainTlsMode,
  verifyCustomDomain,
} from "../lib/orchestratorApi";
import { useAccessToken } from "../lib/useOrchestratorToken";
import { orchestratorUrl } from "../lib/config";

export function useCustomDomains(deploymentId: number | undefined) {
  const token = useAccessToken();
  const url = orchestratorUrl();
  const { data, error, isLoading, mutate } = useSWR(
    token && deploymentId ? ["customDomains", deploymentId, token] : null,
    () => listCustomDomains(url, token!, deploymentId!),
    // Issuance happens in the background and can take a minute (DNS
    // propagation), so poll while the page is open rather than making the
    // operator reload to watch a domain go active.
    { refreshInterval: 10_000 },
  );

  const add = async (
    domain: string,
    kind: "api" | "site",
    tlsMode: "acme" | "upstream" = "acme",
  ) => {
    if (!token || !deploymentId) return;
    await createCustomDomain(url, token, deploymentId, domain, kind, tlsMode);
    await mutate();
  };

  const remove = async (domain: string) => {
    if (!token || !deploymentId) return;
    await deleteCustomDomain(url, token, deploymentId, domain);
    await mutate();
  };

  const retry = async (domain: string) => {
    if (!token || !deploymentId) return;
    await retryCustomDomain(url, token, deploymentId, domain);
    await mutate();
  };

  /**
   * Switch a domain's TLS mode. Going to `acme` orders a certificate on the
   * spot, so the row comes back `pending` and the poll above shows it move to
   * `issuing` then `active`.
   */
  const setTlsMode = async (domain: string, tlsMode: "acme" | "upstream") => {
    if (!token || !deploymentId) return;
    await setCustomDomainTlsMode(url, token, deploymentId, domain, tlsMode);
    await mutate();
  };

  // Returns the probe result so the caller can surface *why* a domain is
  // still pending — the causes (DNS not pointed here, ACME rate limit) are
  // only fixable by the operator.
  const verify = async (domain: string) => {
    if (!token || !deploymentId) return undefined;
    const result = await verifyCustomDomain(url, token, deploymentId, domain);
    await mutate();
    return result;
  };

  return {
    domains: data?.domains,
    targetHost: data?.targetHost,
    routingEnabled: data?.routingEnabled ?? false,
    error,
    isLoading,
    add,
    remove,
    retry,
    setTlsMode,
    verify,
  };
}
