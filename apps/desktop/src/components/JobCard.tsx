import type { ConversionJob } from "../bindings/ConversionJob";
import type { JobStatus } from "../bindings/JobStatus";
import {
  fileNameOf,
  formatBytes,
  formatDuration,
  formatSizeChange,
} from "../format";
import { operationLabelKey, t, tOr } from "../messages";
import { isTerminal } from "../store";

/**
 * Status is communicated by symbol *and* word, never by colour alone — colour
 * is decoration on top of both.
 *
 * `completedWithWarnings` is deliberately styled as the success it is: the file
 * was written and passed every check. Its notes are things the user should know
 * (quality was re-compressed, metadata was dropped), not problems. Painting it
 * amber with a "!" made a working conversion look like a failure.
 */
const STATUS: Record<
  JobStatus,
  { label: string; symbol: string; tone: string }
> = {
  queued: { label: "Queued", symbol: "•", tone: "idle" },
  preparing: { label: "Preparing", symbol: "•", tone: "busy" },
  running: { label: "Running", symbol: "•", tone: "busy" },
  validating: { label: "Verifying", symbol: "•", tone: "busy" },
  completed: { label: "Done", symbol: "✓", tone: "good" },
  completedWithWarnings: { label: "Done", symbol: "✓", tone: "good" },
  failed: { label: "Failed", symbol: "✕", tone: "bad" },
  cancelled: { label: "Cancelled", symbol: "–", tone: "idle" },
};

/** The folder part of a path, for telling the user where the result landed. */
function folderOf(path: string): string {
  const cut =
    path.lastIndexOf("/") === -1
      ? path.lastIndexOf("\\")
      : path.lastIndexOf("/");
  return cut > 0 ? path.slice(0, cut) : path;
}

interface Props {
  job: ConversionJob;
  onCancel?: (jobId: string) => void;
}

export function JobCard({ job, onCancel }: Props) {
  const status = STATUS[job.status];
  const { percent } = job.progress;
  const running = !isTerminal(job);

  return (
    <article className="card job" aria-labelledby={`job-${job.id}-title`}>
      <header className="job__header">
        <h3 className="job__title" id={`job-${job.id}-title`}>
          {tOr(operationLabelKey(job.operationId), job.operationId)}
        </h3>
        <span className={`badge badge--${status.tone}`}>
          <span aria-hidden="true">{status.symbol}</span> {status.label}
        </span>
      </header>

      {running && (
        <div className="job__progress">
          <div
            className="meter"
            role="progressbar"
            aria-label={t(job.progress.messageKey)}
            aria-valuemin={0}
            aria-valuemax={100}
            {...(percent === null
              ? {}
              : { "aria-valuenow": Math.round(percent) })}
          >
            <div
              className={
                percent === null
                  ? "meter__fill meter__fill--indeterminate"
                  : "meter__fill"
              }
              style={percent === null ? undefined : { width: `${percent}%` }}
            />
          </div>
          <p className="job__stage">
            {t(job.progress.messageKey)}
            {percent !== null && (
              <span className="muted"> · {Math.round(percent)}%</span>
            )}
          </p>
        </div>
      )}

      {job.result && job.result.outputs.length > 0 && (
        <div className="saved">
          <p className="saved__line">
            <span aria-hidden="true">✓</span> Saved{" "}
            <strong>
              {job.result.outputs.map((o) => fileNameOf(o.path)).join(", ")}
            </strong>
          </p>
          <p className="saved__where">
            in <code>{folderOf(job.result.outputs[0]?.path ?? "")}</code>
          </p>
        </div>
      )}

      {/* "Skip" is a real outcome, not a success: without saying so, a job that
          wrote nothing showed a green tick and "−100%", which reads as a
          spectacular saving rather than a no-op. */}
      {job.result && job.result.outputs.length === 0 && (
        <p className="notice notice--warn">
          Nothing was written — a file with that name already existed and the
          policy was set to Skip.
        </p>
      )}

      {job.result && (
        <dl className="facts">
          <div>
            <dt>Size</dt>
            <dd>{formatBytes(job.result.outputTotalBytes)}</dd>
          </div>
          {job.result.inputTotalBytes > 0 && job.result.outputs.length > 0 && (
            <div>
              <dt>Change</dt>
              <dd>{formatSizeChange(job.result.sizeChangePercent)}</dd>
            </div>
          )}
          <div>
            <dt>Took</dt>
            <dd>{formatDuration(job.result.elapsedMs)}</dd>
          </div>
          <div>
            <dt>Checks</dt>
            <dd>
              {job.result.validationReports.reduce(
                (n, r) => n + r.checks.length,
                0,
              )}{" "}
              passed
            </dd>
          </div>
        </dl>
      )}

      {job.result && job.result.warnings.length > 0 && (
        <details className="notes">
          <summary>What changed ({job.result.warnings.length})</summary>
          <ul aria-label="Notes about this conversion">
            {job.result.warnings.map((warning, index) => (
              <li key={`${warning.messageKey}-${index}`}>
                {t(warning.messageKey)}
              </li>
            ))}
          </ul>
        </details>
      )}

      {/* Cancelling is the user's own decision, not a fault — so it gets a quiet
          line, never a red alert with technical details to decode. */}
      {job.status === "cancelled" && (
        <p className="muted">
          Cancelled — nothing was saved, and your files are untouched.
        </p>
      )}

      {job.status === "failed" && job.error && (
        <div className="notice notice--bad" role="alert">
          <p>{t(job.error.messageKey)}</p>
          <p className="muted">
            {job.error.sourceSafe
              ? "Your original files were not changed."
              : "Check your original files."}{" "}
            {job.error.partialOutputRemoved &&
              "The incomplete result was deleted."}
          </p>
          <details>
            <summary>Technical details</summary>
            <pre>{`${job.error.code}: ${job.error.detail}`}</pre>
          </details>
        </div>
      )}

      {running && onCancel && (
        <div className="job__actions">
          <button
            type="button"
            className="btn btn--quiet"
            onClick={() => onCancel(job.id)}
          >
            Cancel
          </button>
        </div>
      )}
    </article>
  );
}
