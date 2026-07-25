import { create } from "zustand";

import type { ConversionJob } from "./bindings/ConversionJob";

/** Terminal statuses, mirroring `JobStatus::is_terminal` in the core crate. */
const TERMINAL: ReadonlySet<ConversionJob["status"]> = new Set([
  "completed",
  "completedWithWarnings",
  "failed",
  "cancelled",
]);

export function isTerminal(job: ConversionJob): boolean {
  return TERMINAL.has(job.status);
}

interface JobState {
  jobs: ConversionJob[];
  setJobs: (jobs: ConversionJob[]) => void;
  /** Applied on every `job-updated` event. */
  upsertJob: (job: ConversionJob) => void;
}

/** Newest first, matching the order the backend returns. */
function byNewest(a: ConversionJob, b: ConversionJob): number {
  return b.createdAt.localeCompare(a.createdAt);
}

export const useJobStore = create<JobState>((set) => ({
  jobs: [],
  setJobs: (jobs) => set({ jobs: [...jobs].sort(byNewest) }),
  upsertJob: (job) =>
    set((state) => {
      const index = state.jobs.findIndex((existing) => existing.id === job.id);
      if (index === -1) return { jobs: [job, ...state.jobs].sort(byNewest) };
      const jobs = [...state.jobs];
      jobs[index] = job;
      return { jobs };
    }),
}));

export function activeJobs(jobs: ConversionJob[]): ConversionJob[] {
  return jobs.filter((job) => !isTerminal(job));
}
