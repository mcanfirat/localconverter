import { open } from "@tauri-apps/plugin-dialog";
import { useCallback, useState } from "react";

import type { ConversionError } from "../bindings/ConversionError";
import type { OverwritePolicy } from "../bindings/OverwritePolicy";
import type { TabularFormat } from "../bindings/TabularFormat";
import {
  ActionBar,
  DropZone,
  PickedFiles,
  useFileDrop,
} from "../components/FilePicker";
import { JobCard } from "../components/JobCard";
import { fileNameOf } from "../format";
import { startJob, toConversionError } from "../ipc";
import { t } from "../messages";
import { useJobStore } from "../store";

const INPUT_EXTENSIONS = ["csv", "tsv", "xlsx", "json"];

const FORMATS: { value: TabularFormat; label: string }[] = [
  { value: "xlsx", label: "XLSX" },
  { value: "csv", label: "CSV" },
  { value: "tsv", label: "TSV" },
  { value: "json", label: "JSON" },
];

export function DataRoute() {
  const [path, setPath] = useState<string | null>(null);
  const [destination, setDestination] = useState<string | null>(null);
  const [format, setFormat] = useState<TabularFormat>("xlsx");
  const [hasHeader, setHasHeader] = useState(true);
  const [policy, setPolicy] = useState<OverwritePolicy>("rename");
  // Column typing is keyed by header name. Left empty, everything stays Text —
  // the safe default that never mangles identifiers.
  const [columnTypes] = useState<Record<string, string>>({});
  const [error, setError] = useState<ConversionError | null>(null);
  const [lastJobId, setLastJobId] = useState<string | null>(null);

  const jobs = useJobStore((state) => state.jobs);
  const lastJob = jobs.find((job) => job.id === lastJobId);

  // One file at a time: delimiter, header and per-column typing are decisions
  // about this particular file, and a batch would need them per row.
  const addPaths = useCallback((incoming: string[]) => {
    if (incoming[0]) setPath(incoming[0]);
  }, []);

  const dragging = useFileDrop(INPUT_EXTENSIONS, addPaths);

  async function chooseDestination() {
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked === "string") setDestination(picked);
  }

  async function run() {
    if (!path || !destination) return;
    setError(null);
    try {
      const job = await startJob({
        operationId: "spreadsheet.convert",
        inputPaths: [path],
        outputDirectory: destination,
        overwritePolicy: policy,
        options: { targetFormat: format, hasHeader, columnTypes },
      });
      setLastJobId(job.id);
    } catch (thrown) {
      setError(toConversionError(thrown));
    }
  }

  const canRun = path !== null && destination !== null;

  return (
    <div className="split">
      <div className="stack">
        <DropZone
          title="Drop a spreadsheet here"
          hint="or click to browse — CSV, TSV, XLSX, JSON"
          filterName="Spreadsheets & data"
          extensions={INPUT_EXTENSIONS}
          multiple={false}
          empty={path === null}
          dragging={dragging}
          onAdd={addPaths}
        />

        <PickedFiles
          rows={path ? [{ name: fileNameOf(path) }] : []}
          noun="file"
          onRemove={() => setPath(null)}
        />
      </div>

      <div className="stack">
        <section className="card">
          <fieldset className="field">
            <legend className="field__label">Convert to</legend>
            <div className="chips">
              {FORMATS.map((option) => (
                <label key={option.value} className="chip">
                  <input
                    type="radio"
                    name="format"
                    checked={format === option.value}
                    onChange={() => setFormat(option.value)}
                  />
                  <span>{option.label}</span>
                </label>
              ))}
            </div>
          </fieldset>

          <label className="field__row">
            <input
              type="checkbox"
              checked={hasHeader}
              onChange={(event) => setHasHeader(event.target.checked)}
            />
            <span>First row is a header</span>
          </label>

          <p className="muted">
            Values are kept exactly as written — <code>007</code> stays{" "}
            <code>007</code>, and long numbers never become <code>1.2E+15</code>
            . Columns are converted as text, which is the safe default. To
            coerce a column to a number or boolean, use the command line:{" "}
            <code>
              localconvert spreadsheet data.csv --to xlsx --column age:number
            </code>
          </p>
        </section>

        <section className="card">
          <fieldset className="field">
            <legend className="field__label">If a file already exists</legend>
            <div className="chips">
              {(
                [
                  ["rename", "Rename"],
                  ["fail", "Stop"],
                  ["skip", "Skip"],
                  ["overwrite", "Replace"],
                ] as const
              ).map(([value, label]) => (
                <label key={value} className="chip">
                  <input
                    type="radio"
                    name="policy"
                    checked={policy === value}
                    onChange={() => setPolicy(value)}
                  />
                  <span>{label}</span>
                </label>
              ))}
            </div>
          </fieldset>
        </section>

        <div className="field">
          <span className="field__label">Save to</span>
          <div className="field__row">
            <button
              type="button"
              className="btn"
              onClick={() => void chooseDestination()}
            >
              {destination ? "Change…" : "Choose folder…"}
            </button>
            <span className="muted">
              {destination ? fileNameOf(destination) : "No folder chosen yet"}
            </span>
          </div>
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

        {lastJob && <JobCard job={lastJob} />}
      </div>

      <ActionBar
        summary={
          path ? (
            <>
              <strong>{fileNameOf(path)}</strong> → {format.toUpperCase()}
              {destination
                ? ` · saving to ${fileNameOf(destination)}`
                : " · choose a folder"}
            </>
          ) : (
            "Drop a spreadsheet above, or click to browse."
          )
        }
        {...(path ? { onClear: () => setPath(null) } : {})}
        onRun={() => void run()}
        runLabel="Convert"
        canRun={canRun}
      />
    </div>
  );
}
