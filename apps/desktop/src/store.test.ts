import { beforeEach, describe, expect, it } from "vitest";

import type { ConversionJob } from "./bindings/ConversionJob";
import type { JobStatus } from "./bindings/JobStatus";
import { activeJobs, isTerminal, useJobStore } from "./store";

function job(overrides: Partial<ConversionJob> = {}): ConversionJob {
  return {
    id: crypto.randomUUID(),
    operationId: "diagnostics.selftest",
    inputFiles: [],
    outputDirectory: "/tmp/out",
    overwritePolicy: "fail",
    options: null,
    status: "queued",
    progress: {
      stage: "queued",
      completedUnits: null,
      totalUnits: null,
      percent: null,
      messageKey: "progress.queued",
    },
    createdAt: new Date().toISOString(),
    startedAt: null,
    completedAt: null,
    result: null,
    error: null,
    ...overrides,
  };
}

describe("job store", () => {
  beforeEach(() => {
    useJobStore.setState({ jobs: [] });
  });

  it("adds a job it has not seen before", () => {
    const a = job();
    useJobStore.getState().upsertJob(a);
    expect(useJobStore.getState().jobs).toHaveLength(1);
  });

  it("replaces rather than duplicates on repeated updates", () => {
    const a = job();
    const store = useJobStore.getState();
    store.upsertJob(a);
    store.upsertJob({ ...a, status: "running" });
    store.upsertJob({ ...a, status: "completed" });

    const jobs = useJobStore.getState().jobs;
    expect(jobs).toHaveLength(1);
    expect(jobs[0]?.status).toBe("completed");
  });

  it("keeps the newest job first", () => {
    const older = job({ createdAt: "2026-01-01T00:00:00.000Z" });
    const newer = job({ createdAt: "2026-06-01T00:00:00.000Z" });
    const store = useJobStore.getState();
    store.upsertJob(older);
    store.upsertJob(newer);

    expect(useJobStore.getState().jobs[0]?.id).toBe(newer.id);
  });

  it("agrees with the Rust definition of a terminal status", () => {
    const terminal: JobStatus[] = ["completed", "completedWithWarnings", "failed", "cancelled"];
    const live: JobStatus[] = ["queued", "preparing", "running", "validating"];

    for (const status of terminal) expect(isTerminal(job({ status }))).toBe(true);
    for (const status of live) expect(isTerminal(job({ status }))).toBe(false);
  });

  it("counts only live jobs as active", () => {
    const jobs = [job({ status: "running" }), job({ status: "completed" }), job()];
    expect(activeJobs(jobs)).toHaveLength(2);
  });
});
