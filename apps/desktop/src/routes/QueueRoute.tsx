import { useState } from "react";

import type { ConversionError } from "../bindings/ConversionError";
import { JobCard } from "../components/JobCard";
import { cancelJob, clearCompletedJobs, toConversionError } from "../ipc";
import { t } from "../messages";
import { isTerminal, useJobStore } from "../store";

export function QueueRoute() {
  const jobs = useJobStore((state) => state.jobs);
  const setJobs = useJobStore((state) => state.setJobs);
  const [error, setError] = useState<ConversionError | null>(null);

  const finished = jobs.filter(isTerminal);

  async function cancel(jobId: string) {
    try {
      await cancelJob(jobId);
    } catch (thrown) {
      setError(toConversionError(thrown));
    }
  }

  async function clear() {
    try {
      setJobs(await clearCompletedJobs());
    } catch (thrown) {
      setError(toConversionError(thrown));
    }
  }

  return (
    <div className="stack">
      <div className="row row--between">
        <h2>Queue</h2>
        <button
          type="button"
          className="btn btn--quiet"
          disabled={finished.length === 0}
          onClick={() => void clear()}
        >
          Clear finished ({finished.length})
        </button>
      </div>

      {error && (
        <div className="notice notice--bad" role="alert">
          {t(error.messageKey)}
        </div>
      )}

      {jobs.length === 0 ? (
        <p className="muted">No jobs yet. Start one from the Convert tab.</p>
      ) : (
        <div className="stack">
          {jobs.map((job) => (
            <JobCard key={job.id} job={job} onCancel={(id) => void cancel(id)} />
          ))}
        </div>
      )}
    </div>
  );
}
