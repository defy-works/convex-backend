// Every deployment on the instance, with what the database intends next to
// what docker is actually doing, plus the lifecycle actions.

import { useMemo, useState } from "react";
import { ConfirmByName } from "../../components/admin/ConfirmByName";
import { AdminLayout } from "../../components/admin/AdminLayout";
import { useAdminFleet } from "../../hooks/useAdmin";
import { deploymentActions, type FleetEntry } from "../../lib/adminApi";
import { orchestratorUrl } from "../../lib/config";
import { useAccessToken } from "../../lib/useOrchestratorToken";

type Notice = { kind: "ok" | "warn" | "error"; text: string };

export default function AdminDeploymentsPage() {
  const { data, error, isLoading, mutate } = useAdminFleet();
  const token = useAccessToken();
  const url = orchestratorUrl();

  const [driftedOnly, setDriftedOnly] = useState(false);
  const [busyId, setBusyId] = useState<number | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [pendingDelete, setPendingDelete] = useState<FleetEntry | null>(null);

  const rows = useMemo(() => {
    if (!data) return [];
    return driftedOnly
      ? data.deployments.filter((d) => d.drifted)
      : data.deployments;
  }, [data, driftedOnly]);

  async function run(
    entry: FleetEntry,
    label: string,
    fn: () => Promise<{ containerWarning: string | null }>,
  ) {
    if (!token) return;
    setBusyId(entry.id);
    setNotice(null);
    try {
      const res = await fn();
      setNotice(
        res.containerWarning
          ? {
              kind: "warn",
              // Surfaced, never swallowed: "paused, but the container did not
              // stop" is exactly what an operator needs to see.
              text: `${entry.name}: ${label} recorded, but ${res.containerWarning}`,
            }
          : { kind: "ok", text: `${entry.name}: ${label}` },
      );
    } catch (e) {
      setNotice({
        kind: "error",
        text: `${entry.name}: ${label} failed — ${e}`,
      });
    } finally {
      setBusyId(null);
      // Revalidate rather than mutating optimistically: this view exists to
      // show real container state, so it must not display a state it merely
      // hopes for.
      await mutate();
      setPendingDelete(null);
    }
  }

  return (
    <AdminLayout title="Deployments">
      {error ? (
        <p className="mb-4 rounded border border-util-error p-4 text-sm">
          Could not load the fleet: {String(error)}
        </p>
      ) : null}

      {notice ? (
        <p
          role="status"
          className={`mb-4 rounded border p-3 text-sm ${
            notice.kind === "error"
              ? "border-util-error"
              : notice.kind === "warn"
                ? "border-util-warning"
                : "border-util-success"
          }`}
        >
          {notice.text}
        </p>
      ) : null}

      <div className="mb-4 flex flex-wrap items-center gap-4 text-sm">
        <label className="flex items-center gap-2">
          <input
            type="checkbox"
            checked={driftedOnly}
            onChange={(e) => setDriftedOnly(e.target.checked)}
          />
          Only drifted
        </label>
        {data ? (
          <span className="text-content-secondary">
            {data.deployments.length} deployments · {data.driftCount} drifted
          </span>
        ) : null}
        {data && !data.containerStatesAvailable ? (
          <span className="text-content-secondary">
            Container state unavailable — this provisioner does not manage
            containers.
          </span>
        ) : null}
      </div>

      {isLoading ? <p className="text-sm">Loading…</p> : null}

      <div className="overflow-x-auto rounded-lg border">
        <table className="w-full text-left text-sm">
          <thead className="border-b bg-background-secondary">
            <tr>
              <th className="px-4 py-2">Deployment</th>
              <th className="px-4 py-2">Team / Project</th>
              <th className="px-4 py-2">Tier</th>
              <th className="px-4 py-2">Intended</th>
              <th className="px-4 py-2">Actual</th>
              <th className="px-4 py-2">Actions</th>
            </tr>
          </thead>
          <tbody className="divide-y">
            {rows.map((d) => {
              const busy = busyId === d.id;
              const paused = d.intendedState === "paused";
              return (
                <tr
                  key={d.id}
                  className={d.drifted ? "bg-util-warning/10" : ""}
                >
                  <td className="px-4 py-2 font-mono">{d.name}</td>
                  <td className="px-4 py-2">
                    {d.teamSlug} / {d.projectSlug}
                  </td>
                  <td className="px-4 py-2">{d.tier}</td>
                  <td className="px-4 py-2">{d.intendedState}</td>
                  <td className="px-4 py-2">
                    {d.actualState}
                    {d.drifted ? (
                      <span className="ml-2 rounded bg-util-warning/20 px-1.5 py-0.5 text-xs">
                        drifted
                      </span>
                    ) : null}
                  </td>
                  <td className="px-4 py-2">
                    <div className="flex gap-2">
                      <button
                        type="button"
                        disabled={busy || !token}
                        onClick={() =>
                          run(d, paused ? "resumed" : "paused", () =>
                            paused
                              ? deploymentActions.resume(url, token!, d.id)
                              : deploymentActions.pause(url, token!, d.id),
                          )
                        }
                        className="rounded border px-2 py-1 text-xs disabled:opacity-40"
                      >
                        {paused ? "Resume" : "Pause"}
                      </button>
                      <button
                        type="button"
                        disabled={busy || !token}
                        onClick={() =>
                          run(d, "restarted", () =>
                            deploymentActions.restart(url, token!, d.id),
                          )
                        }
                        className="rounded border px-2 py-1 text-xs disabled:opacity-40"
                      >
                        Restart
                      </button>
                      <button
                        type="button"
                        disabled={busy || !token}
                        onClick={() => setPendingDelete(d)}
                        className="rounded border border-util-error px-2 py-1 text-xs disabled:opacity-40"
                      >
                        Delete
                      </button>
                    </div>
                  </td>
                </tr>
              );
            })}
            {rows.length === 0 && !isLoading ? (
              <tr>
                <td
                  colSpan={6}
                  className="px-4 py-6 text-center text-content-secondary"
                >
                  {driftedOnly
                    ? "No drifted deployments."
                    : "No deployments on this instance."}
                </td>
              </tr>
            ) : null}
          </tbody>
        </table>
      </div>

      {pendingDelete ? (
        <ConfirmByName
          title="Delete deployment"
          expected={pendingDelete.name}
          confirmLabel="Delete permanently"
          busy={busyId === pendingDelete.id}
          onCancel={() => setPendingDelete(null)}
          onConfirm={() =>
            run(pendingDelete, "deleted", () =>
              deploymentActions.remove(url, token!, pendingDelete.id),
            )
          }
          description={
            <>
              This tears down the container and removes{" "}
              <span className="font-mono">{pendingDelete.name}</span> from{" "}
              <span className="font-mono">
                {pendingDelete.teamSlug}/{pendingDelete.projectSlug}
              </span>
              . Its data is not recoverable from here.
            </>
          }
        />
      ) : null}
    </AdminLayout>
  );
}
