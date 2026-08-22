// Every team on the instance, with the counts that make a delete legible.

import { useState } from "react";
import { AdminLayout } from "../../components/admin/AdminLayout";
import { ConfirmByName } from "../../components/admin/ConfirmByName";
import { useAdminTeams } from "../../hooks/useAdmin";
import { teamActions, type AdminTeam } from "../../lib/adminApi";
import { orchestratorUrl } from "../../lib/config";
import { useAccessToken } from "../../lib/useOrchestratorToken";

export default function AdminTeamsPage() {
  const { data, error, isLoading, mutate } = useAdminTeams();
  const token = useAccessToken();
  const url = orchestratorUrl();

  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [pendingDelete, setPendingDelete] = useState<AdminTeam | null>(null);
  const [newName, setNewName] = useState("");

  async function withBusy(fn: () => Promise<string>) {
    if (!token) return;
    setBusy(true);
    setNotice(null);
    try {
      setNotice(await fn());
    } catch (e) {
      setNotice(`Failed: ${e}`);
    } finally {
      setBusy(false);
      await mutate();
      setPendingDelete(null);
    }
  }

  return (
    <AdminLayout title="Teams">
      {error ? (
        <p className="mb-4 rounded border border-util-error p-4 text-sm">
          Could not load teams: {String(error)}
        </p>
      ) : null}
      {notice ? (
        <p role="status" className="mb-4 rounded border p-3 text-sm">
          {notice}
        </p>
      ) : null}

      <form
        className="mb-6 flex gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          const name = newName.trim();
          if (!name) return;
          void withBusy(async () => {
            const r = await teamActions.create(url, token!, name);
            setNewName("");
            return `Created ${r.slug}`;
          });
        }}
      >
        <input
          value={newName}
          onChange={(e) => setNewName(e.target.value)}
          placeholder="New team name"
          aria-label="New team name"
          className="rounded border px-2 py-1 text-sm"
        />
        <button
          type="submit"
          disabled={busy || !newName.trim()}
          className="rounded border px-3 py-1 text-sm disabled:opacity-40"
        >
          Create team
        </button>
      </form>

      {isLoading ? <p className="text-sm">Loading…</p> : null}

      <div className="overflow-x-auto rounded-lg border">
        <table className="w-full text-left text-sm">
          <thead className="border-b bg-background-secondary">
            <tr>
              <th className="px-4 py-2">Team</th>
              <th className="px-4 py-2">Slug</th>
              <th className="px-4 py-2">Members</th>
              <th className="px-4 py-2">Projects</th>
              <th className="px-4 py-2">Deployments</th>
              <th className="px-4 py-2">Actions</th>
            </tr>
          </thead>
          <tbody className="divide-y">
            {(data?.teams ?? []).map((t) => (
              <tr key={t.id}>
                <td className="px-4 py-2">{t.name}</td>
                <td className="px-4 py-2 font-mono">{t.slug}</td>
                <td className="px-4 py-2">{t.memberCount}</td>
                <td className="px-4 py-2">{t.projectCount}</td>
                <td className="px-4 py-2">{t.deploymentCount}</td>
                <td className="px-4 py-2">
                  <button
                    type="button"
                    disabled={busy || !token}
                    onClick={() => setPendingDelete(t)}
                    className="rounded border border-util-error px-2 py-1 text-xs disabled:opacity-40"
                  >
                    Delete
                  </button>
                </td>
              </tr>
            ))}
            {data && data.teams.length === 0 && !isLoading ? (
              <tr>
                <td
                  colSpan={6}
                  className="px-4 py-6 text-center text-content-secondary"
                >
                  No teams on this instance.
                </td>
              </tr>
            ) : null}
          </tbody>
        </table>
      </div>

      {pendingDelete ? (
        <ConfirmByName
          title="Delete team"
          // The slug, not the display name: it is the stable identifier and
          // it is what the operator sees in URLs and deploy keys.
          expected={pendingDelete.slug}
          confirmLabel="Delete team and everything in it"
          busy={busy}
          onCancel={() => setPendingDelete(null)}
          onConfirm={() =>
            void withBusy(async () => {
              const r = await teamActions.remove(url, token!, pendingDelete.id);
              return `Deleted ${r.slug}, tearing down ${r.deploymentsRemoved} deployment(s)`;
            })
          }
          description={
            <>
              This deletes{" "}
              <span className="font-mono">{pendingDelete.slug}</span>, its{" "}
              {pendingDelete.projectCount} project(s), and tears down{" "}
              <strong>{pendingDelete.deploymentCount} deployment(s)</strong>.
              Their data is not recoverable from here.
            </>
          }
        />
      ) : null}
    </AdminLayout>
  );
}
