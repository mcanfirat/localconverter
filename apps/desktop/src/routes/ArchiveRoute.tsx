import { open } from "@tauri-apps/plugin-dialog";
import { useCallback, useState } from "react";

import type { ArchiveFormat } from "../bindings/ArchiveFormat";
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

const ARCHIVE_EXTENSIONS = ["zip", "tar", "gz", "tgz"];

const FORMATS: { value: ArchiveFormat; label: string; note: string }[] = [
  { value: "zip", label: "ZIP", note: "Most compatible" },
  { value: "tarGz", label: "TAR.GZ", note: "Common on Linux/macOS" },
  { value: "tar", label: "TAR", note: "No compression" },
];

type Mode = "create" | "extract";

export function ArchiveRoute() {
  const [mode, setMode] = useState<Mode>("create");
  const [paths, setPaths] = useState<string[]>([]);
  const [names, setNames] = useState<string[]>([]);
  const [destination, setDestination] = useState<string | null>(null);
  const [format, setFormat] = useState<ArchiveFormat>("zip");
  const [archiveName, setArchiveName] = useState("archive");
  const [policy, setPolicy] = useState<OverwritePolicy>("rename");
  const [error, setError] = useState<ConversionError | null>(null);
  const [lastJobId, setLastJobId] = useState<string | null>(null);

  const jobs = useJobStore((state) => state.jobs);
  const lastJob = jobs.find((job) => job.id === lastJobId);

  // Extracting reads exactly one archive; creating takes as many as you like.
  const addPaths = useCallback(
    (incoming: string[]) => {
      setPaths((current) => {
        const next =
          mode === "extract"
            ? incoming.slice(0, 1)
            : [...new Set([...current, ...incoming])];
        setNames(next.map(fileNameOf));
        return next;
      });
    },
    [mode],
  );

  // Creating an archive accepts anything, so there is nothing to filter on.
  const dragging = useFileDrop(
    mode === "extract" ? ARCHIVE_EXTENSIONS : [],
    addPaths,
  );

  async function chooseDestination() {
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked === "string") setDestination(picked);
  }

  async function run() {
    if (!destination || paths.length === 0) return;
    setError(null);
    try {
      const job = await startJob(
        mode === "create"
          ? {
              operationId: "archive.create",
              inputPaths: paths,
              outputDirectory: destination,
              overwritePolicy: policy,
              options: { format, archiveName: archiveName.trim() || "archive" },
            }
          : {
              operationId: "archive.extract",
              inputPaths: [paths[0] ?? ""],
              outputDirectory: destination,
              overwritePolicy: policy,
              options: null,
            },
      );
      setLastJobId(job.id);
    } catch (thrown) {
      setError(toConversionError(thrown));
    }
  }

  const canRun = destination !== null && paths.length > 0;

  return (
    <div className="split">
      <div className="stack">
        <DropZone
          title={mode === "create" ? "Drop files here" : "Drop an archive here"}
          hint={
            mode === "create"
              ? "or click to browse — any file, any number"
              : "or click to browse — ZIP, TAR, TAR.GZ"
          }
          filterName="Archives"
          extensions={mode === "extract" ? ARCHIVE_EXTENSIONS : []}
          multiple={mode === "create"}
          empty={paths.length === 0}
          dragging={dragging}
          onAdd={addPaths}
        />

        <PickedFiles
          rows={names.map((name) => ({ name }))}
          noun={mode === "create" ? "file" : "archive"}
          onRemove={(index) => {
            const next = paths.filter((_, i) => i !== index);
            setPaths(next);
            setNames(next.map(fileNameOf));
          }}
        />
      </div>

      <div className="stack">
        <section className="card">
          <fieldset className="field">
            <legend className="field__label">What to do</legend>
            <div className="chips">
              {(
                [
                  ["create", "Create"],
                  ["extract", "Extract"],
                ] as const
              ).map(([value, label]) => (
                <label key={value} className="chip">
                  <input
                    type="radio"
                    name="mode"
                    checked={mode === value}
                    onChange={() => {
                      setMode(value);
                      setPaths([]);
                      setNames([]);
                    }}
                  />
                  <span>{label}</span>
                </label>
              ))}
            </div>
            <p className="muted">
              {mode === "create"
                ? t("operation.archiveCreate.description")
                : t("operation.archiveExtract.description")}
            </p>
          </fieldset>
        </section>

        {mode === "create" && (
          <section className="card">
            <fieldset className="field">
              <legend className="field__label">Archive format</legend>
              <div className="chips">
                {FORMATS.map((option) => (
                  <label
                    key={option.value}
                    className="chip"
                    title={option.note}
                  >
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

            <div className="field">
              <label className="field__label" htmlFor="archive-name">
                Archive name
              </label>
              <div className="field__row">
                <input
                  id="archive-name"
                  type="text"
                  value={archiveName}
                  onChange={(event) => setArchiveName(event.target.value)}
                />
                <span className="muted">
                  .
                  {FORMATS.find((f) => f.value === format)?.value === "tarGz"
                    ? "tar.gz"
                    : format}
                </span>
              </div>
            </div>
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
                {paths.length} {mode === "create" ? "file" : "archive"}
                {paths.length === 1 ? "" : "s"}
              </strong>{" "}
              →{" "}
              {mode === "create"
                ? format === "tarGz"
                  ? "TAR.GZ"
                  : format.toUpperCase()
                : "folder"}
              {destination
                ? ` · saving to ${fileNameOf(destination)}`
                : " · choose a folder"}
            </>
          ) : (
            `Drop ${mode === "create" ? "files" : "an archive"} above, or click to browse.`
          )
        }
        {...(paths.length > 0
          ? {
              onClear: () => {
                setPaths([]);
                setNames([]);
              },
            }
          : {})}
        onRun={() => void run()}
        runLabel={mode === "create" ? "Create archive" : "Extract"}
        canRun={canRun}
      />
    </div>
  );
}
