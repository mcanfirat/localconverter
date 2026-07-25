import { open } from "@tauri-apps/plugin-dialog";
import { useState } from "react";

import type { ArchiveFormat } from "../bindings/ArchiveFormat";
import type { ConversionError } from "../bindings/ConversionError";
import type { OverwritePolicy } from "../bindings/OverwritePolicy";
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

  async function chooseInputs() {
    // exactOptionalPropertyTypes forbids passing `filters: undefined`, so the
    // extract-only filter is spread in conditionally rather than set to undefined.
    const picked = await open({
      multiple: mode === "create",
      ...(mode === "extract"
        ? { filters: [{ name: "Archives", extensions: ARCHIVE_EXTENSIONS }] }
        : {}),
    });
    const list = Array.isArray(picked) ? picked : typeof picked === "string" ? [picked] : [];
    setPaths(list);
    setNames(list.map(fileNameOf));
  }

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
    <div className="stack">
      <section className="card">
        <h2>Archives</h2>
        <div className="chips">
          <label className="chip">
            <input
              type="radio"
              name="mode"
              checked={mode === "create"}
              onChange={() => {
                setMode("create");
                setPaths([]);
                setNames([]);
              }}
            />
            <span>Create</span>
          </label>
          <label className="chip">
            <input
              type="radio"
              name="mode"
              checked={mode === "extract"}
              onChange={() => {
                setMode("extract");
                setPaths([]);
                setNames([]);
              }}
            />
            <span>Extract</span>
          </label>
        </div>
        <p className="muted">
          {mode === "create"
            ? t("operation.archiveCreate.description")
            : t("operation.archiveExtract.description")}
        </p>

        <div className="field__row">
          <button type="button" className="btn" onClick={() => void chooseInputs()}>
            {mode === "create" ? "Choose files…" : "Choose an archive…"}
          </button>
          <button type="button" className="btn" onClick={() => void chooseDestination()}>
            Choose destination…
          </button>
          <span className="muted">
            {destination ? `Saving to ${fileNameOf(destination)}` : "No destination chosen"}
          </span>
        </div>
      </section>

      {names.length > 0 && (
        <section className="card">
          <h2>
            {names.length} {mode === "create" ? "file" : "archive"}
            {names.length === 1 ? "" : "s"} selected
          </h2>
          <ul className="filelist">
            {names.map((name, index) => (
              <li key={`${name}-${index}`} className="filelist__row">
                <span className="filelist__name">{name}</span>
              </li>
            ))}
          </ul>
        </section>
      )}

      {mode === "create" && (
        <section className="card">
          <fieldset className="field">
            <legend className="field__label">Archive format</legend>
            <div className="chips">
              {FORMATS.map((option) => (
                <label key={option.value} className="chip" title={option.note}>
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
                .{FORMATS.find((f) => f.value === format)?.value === "tarGz" ? "tar.gz" : format}
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

      <div className="field__row">
        <button
          type="button"
          className="btn btn--primary"
          disabled={!canRun}
          onClick={() => void run()}
        >
          {mode === "create" ? "Create archive" : "Extract"}
        </button>
        {paths.length === 0 && (
          <span className="muted">
            Choose {mode === "create" ? "some files" : "an archive"} first.
          </span>
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

      {lastJob && <JobCard job={lastJob} />}
    </div>
  );
}
