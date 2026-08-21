// Every deployment on the instance, with what the database intends next to
// what docker is actually doing.
//
// Read-only in Phase 1 — the lifecycle actions land in Phase 2.

import { useMemo, useState } from "react";
import { AdminLayout } from "../../components/admin/AdminLayout";
import { useAdminFleet } from "../../hooks/useAdmin";

export default function AdminDeploymentsPage() {
  const { data, error, isLoading } = useAdminFleet();
  const [driftedOnly, setDriftedOnly] = useState(false);

  const rows = useMemo(() => {
    if (!data) return [];
    return driftedOnly
      ? data.deployments.filter((d) => d.drifted)
      : data.deployments;
  }, [data, driftedOnly]);

  return (
    <AdminLayout title="Deployments">
      {error ? (
        <p className="mb-4 rounded border border-util-error p-4 text-sm">
          Could not load the fleet: {String(error)}
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
              <th className="px-4 py-2">Type</th>
              <th className="px-4 py-2">Tier</th>
              <th className="px-4 py-2">Intended</th>
              <th className="px-4 py-2">Actual</th>
            </tr>
          </thead>
          <tbody className="divide-y">
            {rows.map((d) => (
              <tr key={d.id} className={d.drifted ? "bg-util-warning/10" : ""}>
                <td className="px-4 py-2 font-mono">{d.name}</td>
                <td className="px-4 py-2">
                  {d.teamSlug} / {d.projectSlug}
                </td>
                <td className="px-4 py-2">{d.deploymentType}</td>
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
              </tr>
            ))}
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
    </AdminLayout>
  );
}
