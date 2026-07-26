import { open } from "@tauri-apps/plugin-dialog";
import { useCallback, useState } from "react";

import type { ConversionError } from "../bindings/ConversionError";
import type { OverwritePolicy } from "../bindings/OverwritePolicy";
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

type PdfOp =
  | "pdf.merge"
  | "pdf.pages"
  | "pdf.split"
  | "pdf.removeMetadata"
  | "pdf.fromImages";

const OPS: { value: PdfOp; label: string; multi: boolean; image: boolean }[] = [
  { value: "pdf.merge", label: "Merge", multi: true, image: false },
  {
    value: "pdf.pages",
    label: "Extract / reorder / rotate",
    multi: false,
    image: false,
  },
  { value: "pdf.split", label: "Split into pages", multi: false, image: false },
  {
    value: "pdf.removeMetadata",
    label: "Remove metadata",
    multi: false,
    image: false,
  },
  { value: "pdf.fromImages", label: "Images → PDF", multi: true, image: true },
];

const ROTATIONS = [0, 90, 180, 270] as const;

const DESCRIPTION_KEY: Record<PdfOp, string> = {
  "pdf.merge": "operation.pdfMerge.description",
  "pdf.pages": "operation.pdfPages.description",
  "pdf.split": "operation.pdfSplit.description",
  "pdf.removeMetadata": "operation.pdfRemoveMetadata.description",
  "pdf.fromImages": "operation.pdfFromImages.description",
};

export function PdfRoute() {
  const [op, setOp] = useState<PdfOp>("pdf.merge");
  const [paths, setPaths] = useState<string[]>([]);
  const [destination, setDestination] = useState<string | null>(null);
  const [pageRange, setPageRange] = useState("");
  const [rotate, setRotate] = useState<number>(0);
  const [policy, setPolicy] = useState<OverwritePolicy>("rename");
  const [error, setError] = useState<ConversionError | null>(null);
  const [lastJobId, setLastJobId] = useState<string | null>(null);

  const jobs = useJobStore((state) => state.jobs);
  const lastJob = jobs.find((job) => job.id === lastJobId);
  const current = OPS.find((o) => o.value === op) ?? OPS[0]!;

  // What counts as input depends on the operation: images→PDF takes pictures,
  // everything else takes PDFs, and only merge and images→PDF take more than one.
  const accepted = current.image
    ? ["jpg", "jpeg", "png", "webp", "tiff", "bmp", "gif"]
    : ["pdf"];

  const addPaths = useCallback(
    (incoming: string[]) => {
      setPaths((cur) =>
        current.multi
          ? [...new Set([...cur, ...incoming])]
          : incoming.slice(0, 1),
      );
    },
    [current.multi],
  );

  const dragging = useFileDrop(accepted, addPaths);

  async function chooseDestination() {
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked === "string") setDestination(picked);
  }

  function optionsFor(): {
    pages: string | null;
    rotate: number | null;
  } | null {
    switch (op) {
      case "pdf.pages":
        return {
          pages: pageRange.trim() === "" ? null : pageRange.trim(),
          rotate: rotate === 0 ? null : rotate,
        };
      default:
        return null;
    }
  }

  async function run() {
    if (!destination || paths.length === 0) return;
    setError(null);
    try {
      const job = await startJob({
        operationId: op,
        inputPaths: paths,
        outputDirectory: destination,
        overwritePolicy: policy,
        options: optionsFor(),
      });
      setLastJobId(job.id);
    } catch (thrown) {
      setError(toConversionError(thrown));
    }
  }

  const canRun =
    destination !== null &&
    paths.length > 0 &&
    !(current.multi && op === "pdf.merge" && paths.length < 2);

  return (
    <div className="split">
      <div className="stack">
        <DropZone
          title={
            current.image
              ? "Drop images here"
              : current.multi
                ? "Drop PDFs here"
                : "Drop a PDF here"
          }
          hint={
            current.image
              ? "or click to browse — JPG, PNG, WebP, TIFF, BMP, GIF"
              : "or click to browse — PDF"
          }
          filterName={current.image ? "Images" : "PDF"}
          extensions={accepted}
          multiple={current.multi}
          empty={paths.length === 0}
          dragging={dragging}
          onAdd={addPaths}
        />

        <PickedFiles
          rows={paths.map((path) => ({ name: fileNameOf(path) }))}
          noun="file"
          onRemove={(index) => setPaths(paths.filter((_, i) => i !== index))}
        />

        {op === "pdf.merge" && paths.length === 1 && (
          <p className="notice notice--warn">
            Merging needs at least two PDFs.
          </p>
        )}
      </div>

      <div className="stack">
        <section className="card">
          <fieldset className="field">
            <legend className="field__label">What to do</legend>
            <div className="chips">
              {OPS.map((option) => (
                <label key={option.value} className="chip">
                  <input
                    type="radio"
                    name="pdfop"
                    checked={op === option.value}
                    onChange={() => {
                      setOp(option.value);
                      setPaths([]);
                    }}
                  />
                  <span>{option.label}</span>
                </label>
              ))}
            </div>
            <p className="muted">{t(DESCRIPTION_KEY[op])}</p>
          </fieldset>
        </section>

        {op === "pdf.pages" && (
          <section className="card">
            <div className="field">
              <label className="field__label" htmlFor="range">
                Pages to keep (blank = all)
              </label>
              <input
                id="range"
                type="text"
                placeholder="e.g. 1-3,7,10-12"
                value={pageRange}
                onChange={(event) => setPageRange(event.target.value)}
              />
            </div>
            <fieldset className="field">
              <legend className="field__label">Rotate</legend>
              <div className="chips">
                {ROTATIONS.map((deg) => (
                  <label key={deg} className="chip">
                    <input
                      type="radio"
                      name="rotate"
                      checked={rotate === deg}
                      onChange={() => setRotate(deg)}
                    />
                    <span>{deg}°</span>
                  </label>
                ))}
              </div>
            </fieldset>
          </section>
        )}

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
          paths.length > 0 ? (
            <>
              <strong>
                {paths.length} file{paths.length === 1 ? "" : "s"}
              </strong>{" "}
              → {current.label}
              {destination
                ? ` · saving to ${fileNameOf(destination)}`
                : " · choose a folder"}
            </>
          ) : (
            `Drop ${current.image ? "images" : current.multi ? "PDFs" : "a PDF"} above, or click to browse.`
          )
        }
        {...(paths.length > 0 ? { onClear: () => setPaths([]) } : {})}
        onRun={() => void run()}
        runLabel="Run"
        canRun={canRun}
      />
    </div>
  );
}
