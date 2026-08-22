// Type-the-name confirmation for destructive admin actions.
//
// A misclick on the wrong fleet row destroys a tenant's data, and the rows
// are visually near-identical. Requiring the name typed exactly is the same
// bar the rest of the dashboard uses for deletes.

import { useState } from "react";

export function ConfirmByName({
  title,
  expected,
  description,
  confirmLabel,
  onConfirm,
  onCancel,
  busy,
}: {
  title: string;
  /** The exact string the operator must type — usually the resource name. */
  expected: string;
  description: React.ReactNode;
  confirmLabel: string;
  onConfirm: () => void;
  onCancel: () => void;
  busy?: boolean;
}) {
  const [typed, setTyped] = useState("");
  // Exact match, not trimmed-or-lowercased: the point is deliberate effort.
  const matches = typed === expected;

  return (
    <div
      role="dialog"
      aria-label={title}
      aria-modal="true"
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4"
    >
      <div className="w-full max-w-md rounded-lg border bg-background-primary p-6">
        <h2 className="text-lg font-semibold">{title}</h2>
        <div className="mt-2 text-sm text-content-secondary">{description}</div>

        <label className="mt-4 block text-sm">
          Type <code className="font-mono font-semibold">{expected}</code> to
          confirm
          <input
            autoFocus
            value={typed}
            onChange={(e) => setTyped(e.target.value)}
            disabled={busy}
            className="mt-1 w-full rounded border px-2 py-1 font-mono"
            aria-label={`Type ${expected} to confirm`}
          />
        </label>

        <div className="mt-6 flex justify-end gap-2">
          <button
            type="button"
            onClick={onCancel}
            disabled={busy}
            className="rounded border px-3 py-1.5 text-sm"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={onConfirm}
            disabled={!matches || busy}
            className="rounded bg-util-error px-3 py-1.5 text-sm text-white disabled:opacity-40"
          >
            {busy ? "Working…" : confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
