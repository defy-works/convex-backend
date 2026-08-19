import { useContext, useId, useState } from "react";
import { PermissionsContext } from "@common/lib/deploymentContext";
import { joinUrlPath } from "@common/lib/helpers/joinUrlPath";
import { cn } from "@ui/cn";

/**
 * Upload a `npx convex export` zip and replace this deployment's tables with
 * its contents.
 *
 * Lives here rather than in a dashboard package because both the standalone
 * self-hosted dashboard and the orchestrator dashboard need it, and keeping two
 * copies is what let the feature exist in one and not the other.
 *
 * `POST /api/import?format=zip&mode=replaceAll` is fully synchronous: the
 * backend's `do_import_from_object_key` calls `perform_import` itself and
 * blocks until the restore has committed, then answers `{numWritten}`. So —
 * unlike `POST /api/export/restore/{id}` — this path needs no separate
 * `/api/perform_import` confirmation step.
 */
export function RestoreFromZipDropzone({
  deploymentUrl,
  adminKey,
}: {
  deploymentUrl: string;
  adminKey: string;
}) {
  const { useIsOperationAllowed } = useContext(PermissionsContext);
  const canImport = useIsOperationAllowed("ImportBackups");

  const [dragOver, setDragOver] = useState(false);
  const [uploading, setUploading] = useState(false);
  const [uploadError, setUploadError] = useState<string | null>(null);
  const [uploadProgress, setUploadProgress] = useState(0);
  const [restoredRows, setRestoredRows] = useState<number | null>(null);
  const inputId = useId();

  const upload = async (file: File) => {
    if (!file.name.toLowerCase().endsWith(".zip")) {
      setUploadError("Choose a .zip file produced by `npx convex export`.");
      return;
    }
    setUploadError(null);
    setRestoredRows(null);
    setUploading(true);
    setUploadProgress(0);
    try {
      const url = joinUrlPath(
        deploymentUrl,
        "/api/import?tableName=&format=zip&mode=replaceAll",
      ).toString();
      // XMLHttpRequest rather than fetch: fetch still can't report upload
      // progress, and these zips are large enough that a progress-less spinner
      // reads as a hang.
      const numWritten = await new Promise<number | null>((resolve, reject) => {
        const xhr = new XMLHttpRequest();
        xhr.open("POST", url);
        xhr.setRequestHeader("Authorization", `Convex ${adminKey}`);
        xhr.setRequestHeader("Convex-Client", "dashboard-0.0.0");
        xhr.upload.onprogress = (ev) => {
          if (ev.lengthComputable) {
            setUploadProgress(Math.round((ev.loaded / ev.total) * 100));
          }
        };
        xhr.onerror = () => reject(new Error("network error"));
        xhr.onload = () => {
          if (xhr.status >= 200 && xhr.status < 300) {
            try {
              const body = JSON.parse(xhr.responseText) as {
                numWritten?: number;
              };
              resolve(body.numWritten ?? null);
            } catch {
              // A 2xx with an unparseable body still means the restore ran.
              resolve(null);
            }
          } else {
            reject(new Error(`HTTP ${xhr.status}: ${xhr.responseText}`));
          }
        };
        xhr.send(file);
      });
      setUploadProgress(0);
      // The request only resolves once the import has committed, so this is a
      // real outcome rather than an optimistic one. Without it a multi-GB
      // restore ends by silently returning the UI to its idle state.
      setRestoredRows(numWritten ?? 0);
    } catch (err) {
      setUploadError(err instanceof Error ? err.message : String(err));
    } finally {
      setUploading(false);
    }
  };

  if (canImport === false) {
    return (
      <p className="text-xs text-content-secondary">
        Your admin key does not permit restoring backups.
      </p>
    );
  }

  return (
    <label
      htmlFor={inputId}
      onDragOver={(e) => {
        e.preventDefault();
        setDragOver(true);
      }}
      onDragLeave={() => setDragOver(false)}
      onDrop={async (e) => {
        e.preventDefault();
        setDragOver(false);
        const file = e.dataTransfer.files?.[0];
        if (file) await upload(file);
      }}
      className={cn(
        "flex w-full cursor-pointer flex-col items-center gap-1 rounded-md border border-dashed p-3 text-center text-xs text-content-secondary transition-colors",
        dragOver && "border-content-primary bg-background-tertiary",
      )}
    >
      <span>
        <strong className="text-content-primary">
          Restore from local backup
        </strong>
      </span>
      <span>Drop a .zip here or click to browse</span>
      {uploading && (
        <span className="mt-1 font-medium text-content-primary">
          Uploading… {uploadProgress}%
        </span>
      )}
      {restoredRows !== null && !uploading && (
        <span className="mt-1 font-medium text-content-success">
          Restored {restoredRows.toLocaleString()}{" "}
          {restoredRows === 1 ? "document" : "documents"}.
        </span>
      )}
      {uploadError && (
        <span className="mt-1 text-content-error">{uploadError}</span>
      )}
      <input
        id={inputId}
        type="file"
        accept=".zip,application/zip"
        className="sr-only"
        disabled={uploading}
        onChange={async (e) => {
          const file = e.target.files?.[0];
          if (file) await upload(file);
          e.target.value = "";
        }}
      />
    </label>
  );
}
