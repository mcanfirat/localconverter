import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";

import type { ConversionError } from "../bindings/ConversionError";
import type { OperationDescriptor } from "../bindings/OperationDescriptor";
import type { OverwritePolicy } from "../bindings/OverwritePolicy";
import { JobCard } from "../components/JobCard";
import { fileNameOf } from "../format";
import { listOperations, startJob, toConversionError } from "../ipc";
import { t } from "../messages";
import { useJobStore } from "../store";

const SIZES = [
  { label: "1 MiB", bytes: 1024 * 1024 },
  { label: "8 MiB", bytes: 8 * 1024 * 1024 },
  { label: "32 MiB", bytes: 32 * 1024 * 1024 },
] as const;

const POLICIES: { value: OverwritePolicy; label: string; hint: string }[] = [
  { value: "fail", label: "Stop", hint: "Refuse to touch an existing file" },
  { value: "rename", label: "Rename", hint: "Write result (1).bin instead" },
  { value: "skip", label: "Skip", hint: "Leave the existing file, write nothing" },
  { value: "overwrite", label: "Replace", hint: "Overwrite the existing file" },
];

export function HomeRoute() {
  const [operations, setOperations] = useState<OperationDescriptor[]>([]);
  const [destination, setDestination] = useState<string | null>(null);
  const [sizeBytes, setSizeBytes] = useState<number>(SIZES[0].bytes);
  const [policy, setPolicy] = useState<OverwritePolicy>("fail");
  const [error, setError] = useState<ConversionError | null>(null);
  const [lastJobId, setLastJobId] = useState<string | null>(null);

  const jobs = useJobStore((state) => state.jobs);
  const lastJob = jobs.find((job) => job.id === lastJobId);

  useEffect(() => {
    void listOperations().then(setOperations).catch(() => setOperations([]));
  }, []);

  const selftest = operations.find((op) => op.id === "diagnostics.selftest");

  async function chooseDestination() {
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked === "string") setDestination(picked);
  }

  async function run() {
    if (!destination || !selftest) return;
    setError(null);
    try {
      const job = await startJob({
        operationId: selftest.id,
        inputPaths: [],
        outputDirectory: destination,
        overwritePolicy: policy,
        options: { sizeBytes },
      });
      setLastJobId(job.id);
    } catch (thrown) {
      setError(toConversionError(thrown));
    }
  }

  return (
    <div className="stack">
      <section className="card card--accent">
        <h2>Diagnostics</h2>
        <p>
          Tools for checking that LocalConvert itself is working. Nothing here
          converts your files — use the Convert tab for that.
        </p>
      </section>

      <section className="card">
        <h2>{t("operation.selftest.label")}</h2>
        <p className="muted">{t("operation.selftest.description")}</p>

        <div className="field">
          <span className="field__label" id="dest-label">
            Destination folder
          </span>
          <div className="field__row">
            <button type="button" className="btn" onClick={() => void chooseDestination()}>
              Choose folder…
            </button>
            <output aria-labelledby="dest-label" className="field__value">
              {destination ? fileNameOf(destination) : "None selected"}
            </output>
          </div>
        </div>

        <fieldset className="field">
          <legend className="field__label">Test file size</legend>
          <div className="chips">
            {SIZES.map((size) => (
              <label key={size.bytes} className="chip">
                <input
                  type="radio"
                  name="size"
                  checked={sizeBytes === size.bytes}
                  onChange={() => setSizeBytes(size.bytes)}
                />
                <span>{size.label}</span>
              </label>
            ))}
          </div>
        </fieldset>

        <fieldset className="field">
          <legend className="field__label">If the file already exists</legend>
          <div className="chips">
            {POLICIES.map((option) => (
              <label key={option.value} className="chip" title={option.hint}>
                <input
                  type="radio"
                  name="policy"
                  checked={policy === option.value}
                  onChange={() => setPolicy(option.value)}
                />
                <span>{option.label}</span>
              </label>
            ))}
          </div>
        </fieldset>

        <div className="field__row">
          <button
            type="button"
            className="btn btn--primary"
            disabled={!destination || !selftest}
            onClick={() => void run()}
          >
            Run self-test
          </button>
          {!destination && (
            <span className="muted">Choose a destination folder first.</span>
          )}
        </div>

        {error && (
          <div className="notice notice--bad" role="alert">
            <p>{t(error.messageKey)}</p>
            <details>
              <summary>Technical details</summary>
              <pre>{`${error.code}: ${error.detail}`}</pre>
            </details>
          </div>
        )}
      </section>

      {lastJob && <JobCard job={lastJob} />}
    </div>
  );
}
