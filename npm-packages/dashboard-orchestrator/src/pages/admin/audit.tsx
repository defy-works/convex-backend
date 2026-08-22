// The instance audit log: operator actions that belong to no single team.
//
// Break-glass events are called out, because those are the rows an operator
// reviews after the fact — "who opened a tenant's deployment, and why".

import { AdminLayout } from "../../components/admin/AdminLayout";
import { useAdminAudit } from "../../hooks/useAdmin";

/** Actions that hand over access to a tenant's data. */
const SENSITIVE = new Set(["deploymentAccessGranted"]);

function reasonOf(metadata: unknown): string | null {
  if (metadata && typeof metadata === "object" && "reason" in metadata) {
    const r = (metadata as { reason?: unknown }).reason;
    return typeof r === "string" ? r : null;
  }
  return null;
}

export default function AdminAuditPage() {
  const { data, error, isLoading } = useAdminAudit(200);

  return (
    <AdminLayout title="Audit log">
      {error ? (
        <p className="mb-4 rounded border border-util-error p-4 text-sm">
          Could not load the audit log: {String(error)}
        </p>
      ) : null}
      {isLoading ? <p className="text-sm">Loading…</p> : null}

      <div className="overflow-x-auto rounded-lg border">
        <table className="w-full text-left text-sm">
          <thead className="border-b bg-background-secondary">
            <tr>
              <th className="px-4 py-2">When</th>
              <th className="px-4 py-2">Actor</th>
              <th className="px-4 py-2">Action</th>
              <th className="px-4 py-2">Detail</th>
            </tr>
          </thead>
          <tbody className="divide-y">
            {(data?.events ?? []).map((e) => {
              const sensitive = SENSITIVE.has(e.action);
              const reason = reasonOf(e.metadata);
              return (
                <tr
                  key={e.id}
                  className={sensitive ? "bg-util-warning/10" : ""}
                >
                  <td className="px-4 py-2 whitespace-nowrap text-content-secondary">
                    {new Date(e.creationTime).toLocaleString()}
                  </td>
                  <td className="px-4 py-2">
                    {/* No member means the break-glass bootstrap credential,
                        which has no human behind it — say so rather than
                        rendering an empty cell. */}
                    {e.memberEmail ?? (
                      <span className="text-content-secondary">
                        bootstrap credential
                      </span>
                    )}
                  </td>
                  <td className="px-4 py-2 font-mono">
                    {e.action}
                    {sensitive ? (
                      <span className="ml-2 rounded bg-util-warning/20 px-1.5 py-0.5 text-xs">
                        data access
                      </span>
                    ) : null}
                  </td>
                  <td className="px-4 py-2">
                    {reason ? (
                      <span title={reason}>{reason}</span>
                    ) : (
                      <code className="text-xs text-content-secondary">
                        {JSON.stringify(e.metadata)}
                      </code>
                    )}
                  </td>
                </tr>
              );
            })}
            {data && data.events.length === 0 && !isLoading ? (
              <tr>
                <td
                  colSpan={4}
                  className="px-4 py-6 text-center text-content-secondary"
                >
                  No instance-scoped events recorded yet.
                </td>
              </tr>
            ) : null}
          </tbody>
        </table>
      </div>
    </AdminLayout>
  );
}
