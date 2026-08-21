// Every member on the instance with their team memberships and flags.
//
// Read-only in Phase 1 — grant/revoke and suspend land in Phase 2.

import { AdminLayout } from "../../components/admin/AdminLayout";
import { useAdminMembers } from "../../hooks/useAdmin";

export default function AdminMembersPage() {
  const { data, error, isLoading } = useAdminMembers();
  const members = data?.members ?? [];

  return (
    <AdminLayout title="Members">
      {error ? (
        <p className="mb-4 rounded border border-util-error p-4 text-sm">
          Could not load members: {String(error)}
        </p>
      ) : null}
      {isLoading ? <p className="text-sm">Loading…</p> : null}

      <div className="overflow-x-auto rounded-lg border">
        <table className="w-full text-left text-sm">
          <thead className="border-b bg-background-secondary">
            <tr>
              <th className="px-4 py-2">Email</th>
              <th className="px-4 py-2">Name</th>
              <th className="px-4 py-2">Teams</th>
              <th className="px-4 py-2">Flags</th>
              <th className="px-4 py-2">Joined</th>
            </tr>
          </thead>
          <tbody className="divide-y">
            {members.map((m) => (
              <tr key={m.id}>
                <td className="px-4 py-2 font-mono">{m.primaryEmail}</td>
                <td className="px-4 py-2">{m.name ?? "—"}</td>
                <td className="px-4 py-2">
                  {m.teams.length === 0
                    ? "—"
                    : m.teams
                        .map((t) => `${t.teamSlug} (${t.role})`)
                        .join(", ")}
                </td>
                <td className="px-4 py-2">
                  {m.isSuperAdmin ? (
                    <span className="mr-2 rounded bg-util-accent/20 px-1.5 py-0.5 text-xs">
                      operator
                    </span>
                  ) : null}
                  {m.suspended ? (
                    <span className="rounded bg-util-error/20 px-1.5 py-0.5 text-xs">
                      suspended
                    </span>
                  ) : null}
                  {!m.isSuperAdmin && !m.suspended ? "—" : null}
                </td>
                <td className="px-4 py-2 text-content-secondary">
                  {new Date(m.creationTime).toLocaleDateString()}
                </td>
              </tr>
            ))}
            {members.length === 0 && !isLoading ? (
              <tr>
                <td
                  colSpan={5}
                  className="px-4 py-6 text-center text-content-secondary"
                >
                  No members on this instance.
                </td>
              </tr>
            ) : null}
          </tbody>
        </table>
      </div>
    </AdminLayout>
  );
}
