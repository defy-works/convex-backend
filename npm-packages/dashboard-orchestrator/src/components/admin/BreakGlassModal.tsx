// Break-glass access to a tenant's deployment.
//
// The modal states, before the operator commits, that the tenant will see
// this in their own audit log. An operator should know that going in rather
// than discover it afterwards — that is what makes the audit trail a
// deterrent rather than a trap.

import { useEffect, useState } from "react";
import type { BreakGlassGrant, FleetEntry } from "../../lib/adminApi";

function Countdown({ expiresAt }: { expiresAt: number }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);

  const msLeft = expiresAt - now;
  if (msLeft <= 0) {
    return <span className="text-util-error">expired</span>;
  }
  const total = Math.floor(msLeft / 1000);
  const mm = Math.floor(total / 60);
  const ss = String(total % 60).padStart(2, "0");
  return (
    <span className={msLeft < 60_000 ? "text-util-error" : undefined}>
      {mm}:{ss}
    </span>
  );
}

export function BreakGlassModal({
  deployment,
  grant,
  busy,
  error,
  onConfirm,
  onClose,
}: {
  deployment: FleetEntry;
  /** Set once access has been granted; the modal then shows the key. */
  grant: BreakGlassGrant | null;
  busy?: boolean;
  error?: string | null;
  onConfirm: (reason: string) => void;
  onClose: () => void;
}) {
  const [reason, setReason] = useState("");
  const [copied, setCopied] = useState(false);
  const canConfirm = reason.trim().length > 0 && !busy;

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Break-glass access"
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4"
    >
      <div className="w-full max-w-lg rounded-lg border bg-background-primary p-6">
        <h2 className="text-lg font-semibold">Open {deployment.name}</h2>

        {grant ? (
          <>
            <p className="mt-2 text-sm text-content-secondary">
              A temporary admin key for{" "}
              <span className="font-mono">{deployment.name}</span>. It is shown
              once and expires in{" "}
              <strong>
                <Countdown expiresAt={grant.expiresAt} />
              </strong>
              .
            </p>
            <div className="mt-4 flex items-center gap-2">
              <code className="flex-1 overflow-x-auto rounded border bg-background-secondary p-2 font-mono text-xs">
                {grant.adminKey}
              </code>
              <button
                type="button"
                onClick={() => {
                  void navigator.clipboard?.writeText(grant.adminKey);
                  setCopied(true);
                }}
                className="rounded border px-3 py-1.5 text-sm"
              >
                {copied ? "Copied" : "Copy"}
              </button>
            </div>
            <p className="mt-4 text-xs text-content-secondary">
              Recorded in the instance audit log and in{" "}
              <span className="font-mono">{deployment.teamSlug}</span>&apos;s
              own audit log.
            </p>
            <div className="mt-6 flex justify-end">
              <button
                type="button"
                onClick={onClose}
                className="rounded border px-3 py-1.5 text-sm"
              >
                Done
              </button>
            </div>
          </>
        ) : (
          <>
            <div className="mt-2 rounded border border-util-warning bg-util-warning/10 p-3 text-sm">
              This grants read and write access to{" "}
              <span className="font-mono">{deployment.teamSlug}</span>&apos;s
              data. <strong>They will see this in their own audit log</strong>,
              including the reason you give.
            </div>

            <label className="mt-4 block text-sm">
              Reason
              <input
                autoFocus
                value={reason}
                onChange={(e) => setReason(e.target.value)}
                disabled={busy}
                placeholder="e.g. investigating ticket 4711"
                aria-label="Reason for access"
                className="mt-1 w-full rounded border px-2 py-1"
              />
            </label>

            {error ? (
              <p className="mt-3 rounded border border-util-error p-2 text-sm">
                {error}
              </p>
            ) : null}

            <div className="mt-6 flex justify-end gap-2">
              <button
                type="button"
                onClick={onClose}
                disabled={busy}
                className="rounded border px-3 py-1.5 text-sm"
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={() => onConfirm(reason.trim())}
                disabled={!canConfirm}
                className="rounded bg-util-warning px-3 py-1.5 text-sm disabled:opacity-40"
              >
                {busy ? "Opening…" : "Grant access"}
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
