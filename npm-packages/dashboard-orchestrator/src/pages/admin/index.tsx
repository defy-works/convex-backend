// Instance overview: capacity, deployment counts, subsystem health, and the
// most recent operator actions.

import { AdminLayout } from "../../components/admin/AdminLayout";
import {
  useAdminAudit,
  useAdminHealth,
  useAdminOverview,
} from "../../hooks/useAdmin";

function Card({
  label,
  value,
  hint,
}: {
  label: string;
  value: string;
  hint?: string;
}) {
  return (
    <div className="rounded-lg border p-4">
      <p className="text-xs uppercase tracking-wide text-content-secondary">
        {label}
      </p>
      <p className="mt-1 text-2xl font-semibold">{value}</p>
      {hint ? (
        <p className="mt-1 text-xs text-content-secondary">{hint}</p>
      ) : null}
    </div>
  );
}

function StatusDot({ ok }: { ok: boolean }) {
  return (
    <span
      aria-label={ok ? "healthy" : "unhealthy"}
      className={`inline-block size-2 rounded-full ${
        ok ? "bg-util-success" : "bg-util-error"
      }`}
    />
  );
}

export default function AdminOverviewPage() {
  const { data: overview, error: overviewError } = useAdminOverview();
  const { data: health } = useAdminHealth();
  const { data: audit } = useAdminAudit(10);

  // Guard against a zero-memory reading rather than rendering NaN%.
  const memoryPct =
    overview && overview.totalMemoryMb > 0
      ? `${Math.round(
          (overview.allocatedMemoryMb / overview.totalMemoryMb) * 100,
        )}%`
      : "—";

  return (
    <AdminLayout title="Overview">
      {overviewError ? (
        <p className="mb-4 rounded border border-util-error p-4 text-sm">
          Could not load the instance overview: {String(overviewError)}
        </p>
      ) : null}

      <section className="grid grid-cols-2 gap-4 md:grid-cols-4">
        <Card
          label="Deployments"
          value={overview ? String(overview.deploymentCount) : "—"}
          hint={
            overview
              ? Object.entries(overview.deploymentsByState)
                  .map(([state, n]) => `${n} ${state}`)
                  .join(", ")
              : undefined
          }
        />
        <Card
          label="Teams"
          value={overview ? String(overview.teamCount) : "—"}
        />
        <Card
          label="Members"
          value={overview ? String(overview.memberCount) : "—"}
        />
        <Card
          label="Memory allocated"
          value={memoryPct}
          hint={
            overview
              ? `${overview.allocatedMemoryMb} / ${overview.totalMemoryMb} MB`
              : undefined
          }
        />
      </section>

      <section className="mt-8">
        <h2 className="mb-3 text-lg font-medium">Subsystems</h2>
        <div className="space-y-2 rounded-lg border p-4 text-sm">
          <div className="flex items-center gap-2">
            <StatusDot ok={health?.database.reachable ?? false} />
            <span>
              Postgres
              {health?.database.pingMs != null
                ? ` — ${health.database.pingMs} ms`
                : ""}
            </span>
            {health?.database.error ? (
              <span className="text-content-secondary">
                {health.database.error}
              </span>
            ) : null}
          </div>
          <div className="flex items-center gap-2">
            {/* `null` means this provisioner owns no containers, which is
                not a fault — only an explicit `false` is unhealthy. */}
            <StatusDot ok={health?.provisioner.dockerReachable !== false} />
            <span>
              Provisioner — {health?.provisioner.mode ?? "…"}
              {health?.provisioner.dockerReachable === null
                ? " (does not manage containers)"
                : ""}
            </span>
            {health?.provisioner.error ? (
              <span className="text-content-secondary">
                {health.provisioner.error}
              </span>
            ) : null}
          </div>
          <div className="text-content-secondary">
            Reconcile:{" "}
            {health
              ? health.reconcileIntervalSecs > 0
                ? `every ${health.reconcileIntervalSecs}s`
                : "boot only (periodic reconcile disabled)"
              : "…"}
            {health?.version ? ` · v${health.version}` : ""}
          </div>
        </div>
      </section>

      <section className="mt-8">
        <h2 className="mb-3 text-lg font-medium">Recent operator actions</h2>
        {audit && audit.events.length > 0 ? (
          <ul className="divide-y rounded-lg border text-sm">
            {audit.events.map((e) => (
              <li key={e.id} className="flex justify-between px-4 py-2">
                <span className="font-mono">{e.action}</span>
                <span className="text-content-secondary">
                  {new Date(e.creationTime).toLocaleString()}
                </span>
              </li>
            ))}
          </ul>
        ) : (
          <p className="text-sm text-content-secondary">
            No instance-scoped events recorded yet.
          </p>
        )}
      </section>
    </AdminLayout>
  );
}
