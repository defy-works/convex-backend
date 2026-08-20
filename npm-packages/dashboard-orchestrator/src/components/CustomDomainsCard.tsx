// Custom domain management for a single deployment.
//
// A domain fronts one surface: the Convex API (the database, :3210) or HTTP
// actions (:3211). They're separate hostnames so a deployment can expose
// e.g. api.example.com and hooks.example.com independently.
//
// Validation is always HTTP-01, which needs no credentials, so there is
// nothing else to configure.
//
// A domain can instead be marked `tlsMode: "upstream"`, meaning something in
// front of this orchestrator already terminates TLS. Those domains are routed
// but never sent to ACME, and the renewal sweep skips them — otherwise they
// have no certificate row, so every sweep would find them "due" and flip them
// back to `issuing` forever.
//
// Nothing here claims a domain is live on its own — `certState` reaches
// `active` only after the orchestrator has completed a real HTTPS request
// against the domain. For an upstream domain that request is answered by
// whatever fronts it, so `active` means reachable, not "we serve its cert".

import { useState } from "react";
import { Button } from "@ui/Button";
import { TextInput } from "@ui/TextInput";
import { Sheet } from "@ui/Sheet";
import { Spinner } from "@ui/Spinner";
import { ConfirmationDialog } from "@ui/ConfirmationDialog";
import { CopyButton } from "@common/elements/CopyButton";
import { CheckCircledIcon, TrashIcon } from "@radix-ui/react-icons";
import { useCustomDomains } from "../hooks/useCustomDomains";

export function CustomDomainsCard({
  deploymentId,
  deploymentName,
}: {
  deploymentId: number | undefined;
  deploymentName?: string;
}) {
  const {
    domains,
    targetHost,
    routingEnabled,
    error,
    isLoading,
    add,
    remove,
    retry,
    setTlsMode: applyTlsMode,
    verify,
  } = useCustomDomains(deploymentId);

  const [draft, setDraft] = useState("");
  const [kind, setKind] = useState<"api" | "site">("api");
  const [tlsMode, setTlsMode] = useState<"acme" | "upstream">("acme");
  const [submitting, setSubmitting] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);
  const [pendingDelete, setPendingDelete] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [probeErrors, setProbeErrors] = useState<Record<string, string>>({});

  const onAdd = async (e: React.FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    const domain = draft.trim();
    if (!domain) return;
    setSubmitting(true);
    setFormError(null);
    try {
      await add(domain, kind, tlsMode);
      setDraft("");
    } catch (err) {
      setFormError((err as Error).message);
    } finally {
      setSubmitting(false);
    }
  };

  const onVerify = async (domain: string) => {
    setBusy(domain);
    try {
      const result = await verify(domain);
      setProbeErrors((prev) => ({ ...prev, [domain]: result?.error ?? "" }));
    } catch (err) {
      setProbeErrors((prev) => ({ ...prev, [domain]: (err as Error).message }));
    } finally {
      setBusy(null);
    }
  };

  const onRetry = async (domain: string) => {
    setBusy(domain);
    try {
      await retry(domain);
    } finally {
      setBusy(null);
    }
  };

  /**
   * Take over TLS for a domain that was terminating upstream. Issuance starts
   * immediately, so the row goes pending -> issuing -> active under the poll
   * without the operator having to remove and re-add the domain.
   */
  const onIssueCertificate = async (domain: string) => {
    setBusy(domain);
    setProbeErrors((prev) => ({ ...prev, [domain]: "" }));
    try {
      await applyTlsMode(domain, "acme");
    } catch (err) {
      setProbeErrors((prev) => ({ ...prev, [domain]: (err as Error).message }));
    } finally {
      setBusy(null);
    }
  };

  return (
    <Sheet>
      <h3>Custom Domains</h3>
      {deploymentName && (
        <p className="mt-1 text-xs text-content-secondary">
          Deployment <code>{deploymentName}</code>
        </p>
      )}

      {!routingEnabled && !isLoading && (
        <p className="mt-2 max-w-prose text-sm text-content-warning">
          Custom domain routing is not enabled on this orchestrator. Set{" "}
          <code className="rounded-sm bg-background-tertiary px-1 text-xs">
            CONVEX_ORCHESTRATOR_TRAEFIK_DYNAMIC_DIR
          </code>{" "}
          and restart it — until then a domain added here would be recorded but
          never routed.
        </p>
      )}

      <p className="mt-2 max-w-prose text-sm text-content-secondary">
        Point a CNAME at{" "}
        <code className="rounded-sm bg-background-tertiary px-1 text-xs">
          {targetHost || "your orchestrator host"}
        </code>
        , then add the hostname below. Routing takes effect without a Traefik
        restart, and there is nothing to edit on the server. Certificates are
        issued and renewed automatically unless the hostname is set to terminate
        TLS upstream.
      </p>

      <form onSubmit={onAdd} className="mt-4 flex flex-col gap-3">
        <div className="flex flex-wrap items-end gap-2">
          <div className="min-w-40 grow">
            <TextInput
              id="custom-domain"
              label="Domain"
              placeholder="api.example.com"
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
            />
          </div>
          <label className="flex flex-col gap-1 text-sm">
            <span className="text-content-primary">Points at</span>
            <select
              aria-label="Points at"
              className="h-9 rounded-sm border bg-background-secondary px-2 text-sm"
              value={kind}
              onChange={(e) => setKind(e.target.value as "api" | "site")}
            >
              <option value="api">Database (Convex API)</option>
              <option value="site">HTTP Actions (site)</option>
            </select>
          </label>
          <label className="flex flex-col gap-1 text-sm">
            <span className="text-content-primary">TLS</span>
            <select
              aria-label="TLS"
              className="h-9 rounded-sm border bg-background-secondary px-2 text-sm"
              value={tlsMode}
              onChange={(e) =>
                setTlsMode(e.target.value as "acme" | "upstream")
              }
            >
              <option value="acme">Issue a certificate</option>
              <option value="upstream">Terminated upstream</option>
            </select>
          </label>
          <Button type="submit" disabled={submitting || !draft.trim()}>
            Add
          </Button>
        </div>

        {tlsMode === "upstream" && (
          <p className="max-w-prose text-xs text-content-secondary">
            No certificate is issued or renewed for this hostname — whatever
            sits in front of it (Cloudflare in proxied mode, another reverse
            proxy) is expected to present one. Works with Cloudflare&apos;s{" "}
            <span className="font-semibold">Full</span> SSL mode, which does not
            check the origin certificate. Use{" "}
            <span className="font-semibold">Issue a certificate</span> for{" "}
            <span className="font-semibold">Full (strict)</span>.
          </p>
        )}

        {formError && (
          <p className="text-xs text-content-error" role="alert">
            {formError}
          </p>
        )}
      </form>

      <div className="mt-4 flex flex-col gap-2">
        {isLoading && <Spinner className="size-4" />}
        {error && (
          <p className="text-sm text-content-error">
            Could not load custom domains: {error.message}
          </p>
        )}
        {domains?.length === 0 && (
          <p className="text-sm text-content-secondary">
            No custom domains yet.
          </p>
        )}
        {domains?.map((d) => {
          const probeError = probeErrors[d.domain];
          return (
            <div
              key={d.domain}
              className="flex flex-col gap-1 rounded-sm border p-3"
            >
              <div className="flex flex-wrap items-center gap-2">
                <code className="grow truncate text-sm">{d.domain}</code>
                <span className="text-xs text-content-secondary">
                  {d.kind === "site" ? "HTTP Actions" : "Database"}
                </span>
                {d.tlsMode === "upstream" && (
                  <span
                    className="rounded-sm bg-background-tertiary px-1.5 py-0.5 text-[10px] text-content-secondary uppercase"
                    title="TLS is terminated in front of this orchestrator; no certificate is issued or renewed here."
                  >
                    TLS upstream
                  </span>
                )}
                <CopyButton text={d.domain} />
                <CertStateBadge certState={d.certState} />
                <Button
                  size="xs"
                  variant="neutral"
                  disabled={busy === d.domain}
                  onClick={() => void onVerify(d.domain)}
                >
                  {busy === d.domain ? "Checking…" : "Check"}
                </Button>
                {d.certState === "failed" && d.tlsMode !== "upstream" && (
                  <Button
                    size="xs"
                    variant="neutral"
                    disabled={busy === d.domain}
                    onClick={() => void onRetry(d.domain)}
                  >
                    Retry
                  </Button>
                )}
                {d.tlsMode === "upstream" && (
                  <Button
                    size="xs"
                    variant="neutral"
                    disabled={busy === d.domain}
                    onClick={() => void onIssueCertificate(d.domain)}
                    tip="Stop relying on the upstream terminator and have this orchestrator issue and renew a Let's Encrypt certificate for the domain. Starts immediately."
                  >
                    {busy === d.domain ? "Starting…" : "Issue certificate"}
                  </Button>
                )}
                <Button
                  size="xs"
                  variant="danger"
                  icon={<TrashIcon />}
                  aria-label={`Remove ${d.domain}`}
                  onClick={() => setPendingDelete(d.domain)}
                />
              </div>
              {(probeError || d.lastError) && (
                <p className="text-xs text-content-error">
                  {probeError || d.lastError}
                </p>
              )}
            </div>
          );
        })}
      </div>

      {pendingDelete && (
        <ConfirmationDialog
          onClose={() => setPendingDelete(null)}
          onConfirm={async () => {
            await remove(pendingDelete);
            setPendingDelete(null);
          }}
          confirmText="Remove"
          variant="danger"
          dialogTitle="Remove custom domain"
          dialogBody={`${pendingDelete} will stop routing to this deployment as soon as Traefik reloads, and its certificate will be deleted.`}
        />
      )}
    </Sheet>
  );
}

function CertStateBadge({ certState }: { certState: string }) {
  if (certState === "active") {
    return (
      <span className="flex items-center gap-1 text-xs text-content-success">
        <CheckCircledIcon />
        Active
      </span>
    );
  }
  if (certState === "issuing") {
    return (
      <span className="flex items-center gap-1 text-xs text-content-secondary">
        <Spinner className="size-3" />
        Issuing
      </span>
    );
  }
  if (certState === "failed") {
    return <span className="text-xs text-content-error">Failed</span>;
  }
  return <span className="text-xs text-content-secondary">Pending</span>;
}
