// Every member on the instance, with their team memberships, flags, and
// the governance actions.

import { useState } from "react";
import { AdminLayout } from "../../components/admin/AdminLayout";
import { useAdminMembers } from "../../hooks/useAdmin";
import { memberActions, type AdminMember } from "../../lib/adminApi";
import { orchestratorUrl } from "../../lib/config";
import {
  useAccessToken,
  useOrchestratorSession,
} from "../../lib/useOrchestratorToken";

export default function AdminMembersPage() {
  const { data, error, isLoading, mutate } = useAdminMembers();
  const token = useAccessToken();
  const url = orchestratorUrl();
  const { data: session } = useOrchestratorSession();

  const [busyId, setBusyId] = useState<number | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const members = data?.members ?? [];

  async function run(m: AdminMember, fn: () => Promise<unknown>) {
    if (!token) return;
    setBusyId(m.id);
    setNotice(null);
    try {
      await fn();
    } catch (e) {
      // The 409 from revoking the last operator carries a message that says
      // what to do about it, so show it rather than a generic failure.
      setNotice(String(e));
    } finally {
      setBusyId(null);
      await mutate();
    }
  }

  function confirmSelfSuspend(m: AdminMember): boolean {
    if (session?.memberId !== m.id) return true;
    return window.confirm(
      "This suspends your own account and signs you out. You can recover by " +
        "signing in with BOOTSTRAP_TOKEN. Continue?",
    );
  }

  return (
    <AdminLayout title="Members">
      {error ? (
        <p className="mb-4 rounded border border-util-error p-4 text-sm">
          Could not load members: {String(error)}
        </p>
      ) : null}
      {notice ? (
        <p
          role="status"
          className="mb-4 rounded border border-util-warning p-3 text-sm"
        >
          {notice}
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
              <th className="px-4 py-2">Actions</th>
            </tr>
          </thead>
          <tbody className="divide-y">
            {members.map((m) => {
              const busy = busyId === m.id;
              const isSelf = session?.memberId === m.id;
              return (
                <tr key={m.id}>
                  <td className="px-4 py-2 font-mono">
                    {m.primaryEmail}
                    {isSelf ? (
                      <span className="ml-2 text-xs text-content-secondary">
                        (you)
                      </span>
                    ) : null}
                  </td>
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
                  <td className="px-4 py-2">
                    <div className="flex gap-2">
                      <button
                        type="button"
                        disabled={busy || !token}
                        onClick={() => {
                          if (!m.suspended && !confirmSelfSuspend(m)) return;
                          void run(m, () =>
                            m.suspended
                              ? memberActions.unsuspend(url, token!, m.id)
                              : memberActions.suspend(url, token!, m.id),
                          );
                        }}
                        className="rounded border px-2 py-1 text-xs disabled:opacity-40"
                      >
                        {m.suspended ? "Unsuspend" : "Suspend"}
                      </button>
                      <button
                        type="button"
                        disabled={busy || !token}
                        onClick={() =>
                          void run(m, () =>
                            memberActions.setSuperAdmin(
                              url,
                              token!,
                              m.id,
                              !m.isSuperAdmin,
                            ),
                          )
                        }
                        className="rounded border px-2 py-1 text-xs disabled:opacity-40"
                      >
                        {m.isSuperAdmin ? "Revoke operator" : "Make operator"}
                      </button>
                    </div>
                  </td>
                </tr>
              );
            })}
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
