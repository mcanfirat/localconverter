import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import type { ConversionJob } from "../bindings/ConversionJob";
import { JobCard } from "./JobCard";

function job(overrides: Partial<ConversionJob> = {}): ConversionJob {
  return {
    id: "11111111-2222-3333-4444-555555555555",
    operationId: "diagnostics.selftest",
    inputFiles: [],
    outputDirectory: "/tmp/out",
    overwritePolicy: "fail",
    options: null,
    status: "running",
    progress: {
      stage: "running",
      completedUnits: 5,
      totalUnits: 10,
      percent: 50,
      messageKey: "progress.writing",
    },
    createdAt: "2026-07-24T10:00:00.000Z",
    startedAt: "2026-07-24T10:00:00.000Z",
    completedAt: null,
    result: null,
    error: null,
    ...overrides,
  };
}

describe("JobCard", () => {
  it("shows real progress with an accessible value", () => {
    render(<JobCard job={job()} />);
    const bar = screen.getByRole("progressbar");
    expect(bar).toHaveAttribute("aria-valuenow", "50");
    expect(screen.getByText(/Writing output/)).toBeInTheDocument();
  });

  it("reports indeterminate progress without inventing a number", () => {
    render(
      <JobCard
        job={job({
          progress: {
            stage: "validating",
            completedUnits: null,
            totalUnits: null,
            percent: null,
            messageKey: "progress.validating",
          },
        })}
      />,
    );
    expect(screen.getByRole("progressbar")).not.toHaveAttribute("aria-valuenow");
  });

  it("states status in words, not colour alone", () => {
    render(<JobCard job={job({ status: "completed" })} />);
    expect(screen.getByText(/Done/)).toBeInTheDocument();
  });

  it("shows a job that finished with notes as the success it is", () => {
    // A conversion that wrote a verified file is a success even when there are
    // things to tell the user. Rendering it as an alarm made people think the
    // app was broken.
    render(<JobCard job={job({ status: "completedWithWarnings" })} />);
    expect(screen.getByText(/Done/)).toBeInTheDocument();
    expect(screen.queryByText(/Completed with warnings/)).not.toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("tells the user their source is safe when a job fails", () => {
    render(
      <JobCard
        job={job({
          status: "failed",
          completedAt: "2026-07-24T10:00:05.000Z",
          error: {
            code: "outputValidationFailed",
            messageKey: "error.outputValidationFailed",
            detail: "failed checks: selftest.contentMatches",
            sourceSafe: true,
            partialOutputRemoved: true,
          },
        })}
      />,
    );

    expect(screen.getByRole("alert")).toHaveTextContent(
      "The result failed verification, so it was not kept.",
    );
    expect(screen.getByText(/Your original files were not changed/)).toBeInTheDocument();
    expect(screen.getByText(/The incomplete result was deleted/)).toBeInTheDocument();
  });

  it("renders warnings from a completed job", () => {
    render(
      <JobCard
        job={job({
          status: "completedWithWarnings",
          result: {
            outputs: [
              {
                path: "/tmp/out/localconvert-selftest.bin",
                displayName: "localconvert-selftest.bin",
                sizeBytes: 1024,
                format: "bin",
              },
            ],
            warnings: [{ messageKey: "warning.destination.overwritten", detail: null }],
            validationReports: [],
            elapsedMs: 120,
            inputTotalBytes: 0,
            outputTotalBytes: 1024,
            sizeChangePercent: 0,
          },
        })}
      />,
    );

    // Notes live behind a "what changed" disclosure; they are still rendered.
    expect(screen.getByText("An existing file was replaced.")).toBeInTheDocument();
    expect(screen.getByText("1.00 KiB")).toBeInTheDocument();
    // The saved filename is shown prominently so the user can find the result.
    expect(screen.getByText("localconvert-selftest.bin")).toBeInTheDocument();
    expect(screen.getByText(/Saved/)).toBeInTheDocument();
  });

  it("offers cancel only while the job is live", async () => {
    const onCancel = vi.fn();
    const { rerender } = render(<JobCard job={job()} onCancel={onCancel} />);

    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onCancel).toHaveBeenCalledWith("11111111-2222-3333-4444-555555555555");

    rerender(<JobCard job={job({ status: "completed" })} onCancel={onCancel} />);
    expect(screen.queryByRole("button", { name: "Cancel" })).not.toBeInTheDocument();
  });
});
