// Which hostname a deployment advertises as its own.
//
// Attaching a custom domain only routes traffic. The backend still reports
// whatever CONVEX_CLOUD_ORIGIN / CONVEX_SITE_ORIGIN it was started with, and
// that is what `CONVEX_SITE_URL`, generated HTTP action URLs, and OAuth /
// auth callback URLs are built from. Picking a canonical URL here is what
// makes the backend agree with the domain.
//
// Those origins are baked into the container environment, so a change only
// lands when the backend container is recreated. Nothing here restarts
// anything on its own — it saves the choice and offers the restart.

import { useEffect, useState } from "react";
import { useSWRConfig } from "swr";
import { Button } from "@ui/Button";
import { Sheet } from "@ui/Sheet";
import { Spinner } from "@ui/Spinner";
import { ConfirmationDialog } from "@ui/ConfirmationDialog";
import { useCanonicalUrls } from "../hooks/useCanonicalUrls";
import { useCustomDomains } from "../hooks/useCustomDomains";
import { useAccessToken } from "../lib/useOrchestratorToken";
import { orchestratorUrl } from "../lib/config";
import { restartDeployment } from "../lib/orchestratorApi";

export function CanonicalUrlsCard({
  deploymentId,
  deploymentName,
}: {
  deploymentId: number | undefined;
  deploymentName: string;
}) {
  const { canonical, error, isLoading, save, refresh } =
    useCanonicalUrls(deploymentId);
  const { domains } = useCustomDomains(deploymentId);
  const token = useAccessToken();
  const url = orchestratorUrl();
  const { mutate: globalMutate } = useSWRConfig();

  const [cloud, setCloud] = useState<string | null>(null);
  const [site, setSite] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);
  const [restartOpen, setRestartOpen] = useState(false);
  const [restarting, setRestarting] = useState(false);

  // Seed once the server state arrives. Keyed on the deployment so switching
  // between deployments re-seeds, but an unsaved edit isn't clobbered by a
  // background revalidation.
  useEffect(() => {
    if (canonical) {
      setCloud(canonical.desiredUrl ?? canonical.defaultUrl);
      setSite(canonical.desiredSiteUrl ?? canonical.defaultSiteUrl);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [deploymentId, canonical === undefined]);

  if (isLoading || !canonical) {
    return (
      <Sheet>
        <h3>Canonical URLs</h3>
        {error ? (
          <p className="mt-2 text-sm text-content-error">
            Could not load canonical URLs: {error.message}
          </p>
        ) : (
          <Spinner className="mt-2 size-4" />
        )}
      </Sheet>
    );
  }

  // Only a hostname routed to this deployment can be canonical, so the
  // options are the derived default plus attached domains of that kind.
  const optionsFor = (kind: "api" | "site", fallback: string) => [
    fallback,
    ...(domains ?? [])
      .filter((d) => d.kind === kind)
      .map((d) => `https://${d.domain}`),
  ];

  const cloudOptions = optionsFor("api", canonical.defaultUrl);
  const siteOptions = optionsFor("site", canonical.defaultSiteUrl);

  // A saved canonical URL can stop being selectable — deleting the custom
  // domain it names removes it from the options. A <select> whose value isn't
  // in its options silently displays the first one, so without collapsing to
  // the default here the control would show "default" while the draft still
  // held the deleted hostname: `dirty` false, Save greyed out, and no way to
  // correct it from the UI.
  const cloudValue =
    cloud && cloudOptions.includes(cloud) ? cloud : canonical.defaultUrl;
  const siteValue =
    site && siteOptions.includes(site) ? site : canonical.defaultSiteUrl;

  const dirty =
    cloudValue !== (canonical.desiredUrl ?? canonical.defaultUrl) ||
    siteValue !== (canonical.desiredSiteUrl ?? canonical.defaultSiteUrl);

  const onSave = async () => {
    setSaving(true);
    setFormError(null);
    try {
      await save({ url: cloudValue, siteUrl: siteValue });
    } catch (err) {
      setFormError((err as Error).message);
    } finally {
      setSaving(false);
    }
  };

  const onRestart = async () => {
    if (!token) return;
    setRestarting(true);
    setFormError(null);
    try {
      await restartDeployment(url, token, deploymentName);
      // The restart response says nothing about canonical URLs, but it is
      // exactly what makes the pending change live — re-read so the banner
      // stops claiming otherwise and "currently serving" catches up.
      await refresh();
      // Other views render the deployment's URL from the deployments list,
      // which the restart just changed. Drop those entries so they refetch
      // instead of showing the old hostname until a reload.
      await globalMutate(
        (key) => Array.isArray(key) && key[0] === "deployments",
        undefined,
        { revalidate: true },
      );
    } catch (err) {
      setFormError((err as Error).message);
      throw err;
    } finally {
      setRestarting(false);
    }
  };

  return (
    <Sheet>
      <h3>Canonical URLs</h3>
      <p className="mt-2 max-w-prose text-sm text-content-secondary">
        What this deployment reports as its own address. These become{" "}
        <code className="rounded-sm bg-background-tertiary px-1 text-xs">
          CONVEX_CLOUD_ORIGIN
        </code>{" "}
        and{" "}
        <code className="rounded-sm bg-background-tertiary px-1 text-xs">
          CONVEX_SITE_ORIGIN
        </code>
        , which is where{" "}
        <code className="rounded-sm bg-background-tertiary px-1 text-xs">
          CONVEX_SITE_URL
        </code>
        , generated HTTP action URLs, and auth callback URLs come from. The
        default hostname keeps routing either way, so switching never strands a
        client already using it.
      </p>

      <div className="mt-4 flex flex-col gap-4">
        <UrlPicker
          id="canonical-cloud"
          label="Database (Convex API)"
          value={cloudValue}
          onChange={setCloud}
          options={cloudOptions}
          defaultUrl={canonical.defaultUrl}
          current={canonical.currentUrl}
        />
        <UrlPicker
          id="canonical-site"
          label="HTTP Actions"
          value={siteValue}
          onChange={setSite}
          options={siteOptions}
          defaultUrl={canonical.defaultSiteUrl}
          current={canonical.currentSiteUrl}
        />
      </div>

      {formError && (
        <p className="mt-3 text-xs text-content-error" role="alert">
          {formError}
        </p>
      )}

      {canonical.restartPending && !dirty && (
        <div className="mt-4 flex flex-col gap-2 rounded-sm bg-background-tertiary/40 p-3 sm:flex-row sm:items-center sm:justify-between">
          <p className="text-sm text-content-warning">
            Saved, but not live yet — the backend still reports its previous
            URL. Recreating the container applies it, and the deployment is
            offline for the duration.
          </p>
          <Button
            size="xs"
            variant="neutral"
            disabled={restarting}
            onClick={() => setRestartOpen(true)}
          >
            {restarting ? "Restarting…" : "Restart deployment"}
          </Button>
        </div>
      )}

      <div className="mt-4 flex justify-end">
        <Button size="xs" onClick={onSave} disabled={!dirty || saving}>
          {saving ? "Saving…" : "Save"}
        </Button>
      </div>

      {restartOpen && (
        <ConfirmationDialog
          onClose={() => setRestartOpen(false)}
          onConfirm={onRestart}
          confirmText="Restart deployment"
          dialogTitle="Restart deployment"
          variant="primary"
          dialogBody={
            <>
              <p className="text-sm">
                The backend container is recreated with the new origins. Data is
                untouched.
              </p>
              <p className="mt-3 text-sm font-semibold">
                {deploymentName} will be offline for the duration of the
                restart.
              </p>
            </>
          }
        />
      )}
    </Sheet>
  );
}

function UrlPicker({
  id,
  label,
  value,
  onChange,
  options,
  defaultUrl,
  current,
}: {
  id: string;
  label: string;
  value: string;
  onChange: (v: string) => void;
  options: string[];
  defaultUrl: string;
  current: string;
}) {
  return (
    <label className="flex flex-col gap-1 text-sm" htmlFor={id}>
      <span className="text-content-primary">{label}</span>
      <select
        id={id}
        className="h-9 max-w-full rounded-sm border bg-background-secondary px-2 text-sm"
        value={value}
        onChange={(e) => onChange(e.target.value)}
      >
        {options.map((o) => (
          <option key={o} value={o}>
            {o === defaultUrl ? `${o} (default)` : o}
          </option>
        ))}
      </select>
      {current !== value && (
        <span className="text-xs text-content-secondary">
          Currently serving <code>{current}</code>
        </span>
      )}
    </label>
  );
}
