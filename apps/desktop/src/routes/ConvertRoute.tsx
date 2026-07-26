import { open } from "@tauri-apps/plugin-dialog";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { useCallback, useEffect, useMemo, useState } from "react";

import type { Background } from "../bindings/Background";
import type { ConversionError } from "../bindings/ConversionError";
import type { ImageBatchPreflight } from "../bindings/ImageBatchPreflight";
import type { ImageOutputFormat } from "../bindings/ImageOutputFormat";
import type { OverwritePolicy } from "../bindings/OverwritePolicy";
import type { ResizeSpec } from "../bindings/ResizeSpec";
import { JobCard } from "../components/JobCard";
import { fileNameOf, formatBytes } from "../format";
import { preflightImages, startJob, toConversionError } from "../ipc";
import { t } from "../messages";
import { useJobStore } from "../store";

const INPUT_EXTENSIONS = [
  "jpg",
  "jpeg",
  "jpe",
  "png",
  "webp",
  "tif",
  "tiff",
  "bmp",
  "gif",
];

const FORMATS: { value: ImageOutputFormat; label: string; note: string }[] = [
  { value: "jpeg", label: "JPG", note: "Small, lossy, no transparency" },
  { value: "png", label: "PNG", note: "Lossless, keeps transparency" },
  { value: "webp", label: "WebP", note: "Lossless here, keeps transparency" },
  { value: "tiff", label: "TIFF", note: "Lossless, archival" },
  { value: "bmp", label: "BMP", note: "Uncompressed" },
];

const BACKGROUNDS: { label: string; value: Background }[] = [
  { label: "White", value: { r: 255, g: 255, b: 255 } },
  { label: "Black", value: { r: 0, g: 0, b: 0 } },
];

type ResizeMode = "none" | "fit" | "exact";

export function ConvertRoute() {
  const [paths, setPaths] = useState<string[]>([]);
  const [destination, setDestination] = useState<string | null>(null);
  const [format, setFormat] = useState<ImageOutputFormat>("jpeg");
  const [quality, setQuality] = useState(85);
  const [resizeMode, setResizeMode] = useState<ResizeMode>("none");
  const [boxWidth, setBoxWidth] = useState(1920);
  const [boxHeight, setBoxHeight] = useState(1080);
  const [background, setBackground] = useState<Background>({
    r: 255,
    g: 255,
    b: 255,
  });
  const [policy, setPolicy] = useState<OverwritePolicy>("rename");

  const [lastPreflight, setLastPreflight] =
    useState<ImageBatchPreflight | null>(null);
  const [error, setError] = useState<ConversionError | null>(null);
  const [lastJobId, setLastJobId] = useState<string | null>(null);
  const [dragging, setDragging] = useState(false);

  const jobs = useJobStore((state) => state.jobs);
  const lastJob = jobs.find((job) => job.id === lastJobId);

  const options = useMemo(() => {
    const resize: ResizeSpec | null =
      resizeMode === "fit"
        ? { mode: "fit", maxWidth: boxWidth, maxHeight: boxHeight }
        : resizeMode === "exact"
          ? { mode: "exact", width: boxWidth, height: boxHeight }
          : null;
    return {
      targetFormat: format,
      // The backend rejects a quality value on a lossless format rather than
      // ignoring it, so only send one where it actually means something.
      quality: format === "jpeg" ? quality : null,
      resize,
      // Harmless when nothing is transparent — the engine only paints a
      // background when alpha would otherwise be lost.
      background,
    };
  }, [format, quality, resizeMode, boxWidth, boxHeight, background]);

  // Re-inspect whenever the selection or the options change, so warnings are
  // shown before the user commits rather than reported afterwards.
  useEffect(() => {
    if (paths.length === 0) return;
    let cancelled = false;
    preflightImages(paths, options)
      .then((report) => {
        if (!cancelled) setLastPreflight(report);
      })
      .catch((thrown: unknown) => {
        if (!cancelled) setError(toConversionError(thrown));
      });
    return () => {
      cancelled = true;
    };
  }, [paths, options]);

  // Derived rather than cleared in an effect: with nothing selected there is
  // nothing to report, whatever the last response happened to be.
  const preflight = paths.length === 0 ? null : lastPreflight;

  const addPaths = useCallback((incoming: string[]) => {
    // Filter here rather than letting the backend decline them: dropping a
    // folder or a stray .txt alongside photos is an accident, not a request.
    const images = incoming.filter((path) =>
      INPUT_EXTENSIONS.includes(path.split(".").pop()?.toLowerCase() ?? ""),
    );
    if (images.length === 0) return;
    setPaths((current) => [...new Set([...current, ...images])]);
  }, []);

  // Dragging files onto the window is the gesture people reach for first, and
  // it arrives as a webview event rather than an HTML drop — the renderer
  // never sees a real path, so the DOM drag events cannot be used here.
  //
  // Guarded because `getCurrentWebview` throws outright when the Tauri runtime
  // is absent, as it is when the frontend is opened in an ordinary browser
  // during development. Losing drag-and-drop there is a nuisance; taking the
  // whole route down with it was a blank screen.
  useEffect(() => {
    let stop: (() => void) | undefined;
    let cancelled = false;
    try {
      void getCurrentWebview()
        .onDragDropEvent((event) => {
          if (event.payload.type === "over") setDragging(true);
          else if (event.payload.type === "drop") {
            setDragging(false);
            addPaths(event.payload.paths);
          } else setDragging(false);
        })
        .then((unlisten) => {
          if (cancelled) unlisten();
          else stop = unlisten;
        })
        .catch(() => undefined);
    } catch {
      // No Tauri runtime — the browse button still works.
    }
    return () => {
      cancelled = true;
      stop?.();
    };
  }, [addPaths]);

  async function chooseFiles() {
    const picked = await open({
      multiple: true,
      filters: [{ name: "Images", extensions: INPUT_EXTENSIONS }],
    });
    if (Array.isArray(picked)) addPaths(picked);
    else if (typeof picked === "string") addPaths([picked]);
  }

  async function chooseDestination() {
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked === "string") setDestination(picked);
  }

  async function run() {
    if (!destination || paths.length === 0) return;
    setError(null);
    try {
      const job = await startJob({
        operationId: "image.convert",
        inputPaths: paths,
        outputDirectory: destination,
        overwritePolicy: policy,
        options,
      });
      setLastJobId(job.id);
    } catch (thrown) {
      setError(toConversionError(thrown));
    }
  }

  const convertible = preflight?.convertibleCount ?? 0;
  const canRun = destination !== null && convertible > 0;

  const totalBytes =
    preflight?.files.reduce((sum, file) => sum + file.sizeBytes, 0) ?? 0;

  return (
    <div className="split">
      <div className="stack">
        <button
          type="button"
          className={[
            "dropzone",
            dragging ? "dropzone--hot" : "",
            paths.length === 0 ? "dropzone--empty" : "",
          ]
            .filter(Boolean)
            .join(" ")}
          onClick={() => void chooseFiles()}
        >
          <span className="dropzone__icon" aria-hidden="true">
            ⬒
          </span>
          <span className="dropzone__title">
            {dragging ? "Release to add" : "Drop images here"}
          </span>
          <span className="dropzone__hint">
            or click to browse — JPG, PNG, WebP, TIFF, BMP, GIF
          </span>
        </button>

        {preflight && preflight.files.length > 0 && (
          <>
            <div className="row row--between">
              <strong>
                {preflight.files.length} file
                {preflight.files.length === 1 ? "" : "s"}
              </strong>
              <span className="muted">{formatBytes(totalBytes)} total</span>
            </div>
            <ul className="filelist">
              {preflight.files.map((file, index) => (
                <li
                  key={`${file.displayName}-${index}`}
                  className={
                    file.errorMessageKey
                      ? "filelist__row filelist__row--bad"
                      : file.extensionMismatch
                        ? "filelist__row filelist__row--warn"
                        : "filelist__row"
                  }
                >
                  <span className="filelist__thumb" aria-hidden="true">
                    {file.errorMessageKey
                      ? "✕"
                      : file.extensionMismatch
                        ? "⚠"
                        : "▣"}
                  </span>
                  <span className="filelist__text">
                    <span className="filelist__name">{file.displayName}</span>
                    <span className="filelist__meta">
                      {file.errorMessageKey ? (
                        t(file.errorMessageKey)
                      ) : file.extensionMismatch ? (
                        <>
                          Actually {file.detectedFormat.toUpperCase()} —
                          converted as that
                        </>
                      ) : (
                        <>
                          {file.detectedFormat.toUpperCase()} · {file.width}×
                          {file.height} · {formatBytes(file.sizeBytes)}
                          {file.hasAlpha && " · transparent"}
                          {file.isAnimated && " · animated"}
                        </>
                      )}
                    </span>
                  </span>
                  <button
                    type="button"
                    className="filelist__drop"
                    aria-label={`Remove ${file.displayName}`}
                    onClick={() =>
                      setPaths((current) =>
                        current.filter((_, i) => i !== index),
                      )
                    }
                  >
                    ✕
                  </button>
                </li>
              ))}
            </ul>
          </>
        )}
      </div>

      <div className="stack">
        <section className="card">
          <fieldset className="field">
            <legend className="field__label">Convert to</legend>
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
            <p className="muted">
              {FORMATS.find((f) => f.value === format)?.note}
            </p>
          </fieldset>

          {format === "jpeg" && (
            <div className="field">
              <label className="field__label" htmlFor="quality">
                Quality — {quality}
              </label>
              <input
                id="quality"
                type="range"
                min={1}
                max={100}
                value={quality}
                onChange={(event) => setQuality(Number(event.target.value))}
              />
            </div>
          )}

          <fieldset className="field">
            <legend className="field__label">Resize</legend>
            <div className="chips">
              {(
                [
                  ["none", "Keep original"],
                  ["fit", "Fit within"],
                  ["exact", "Exactly"],
                ] as const
              ).map(([value, label]) => (
                <label key={value} className="chip">
                  <input
                    type="radio"
                    name="resize"
                    checked={resizeMode === value}
                    onChange={() => setResizeMode(value)}
                  />
                  <span>{label}</span>
                </label>
              ))}
            </div>
            {resizeMode !== "none" && (
              <div className="field__row">
                <label>
                  <span className="muted">Width </span>
                  <input
                    type="number"
                    min={1}
                    value={boxWidth}
                    onChange={(event) =>
                      setBoxWidth(Number(event.target.value))
                    }
                  />
                </label>
                <label>
                  <span className="muted">Height </span>
                  <input
                    type="number"
                    min={1}
                    value={boxHeight}
                    onChange={(event) =>
                      setBoxHeight(Number(event.target.value))
                    }
                  />
                </label>
                {resizeMode === "fit" && (
                  <span className="muted">
                    Aspect ratio kept; images are never enlarged.
                  </span>
                )}
              </div>
            )}
          </fieldset>

          {preflight?.backgroundRequired && (
            <fieldset className="field">
              <legend className="field__label">
                Background for transparent areas
              </legend>
              <div className="chips">
                {BACKGROUNDS.map((option) => (
                  <label key={option.label} className="chip">
                    <input
                      type="radio"
                      name="background"
                      checked={
                        background.r === option.value.r &&
                        background.g === option.value.g &&
                        background.b === option.value.b
                      }
                      onChange={() => setBackground(option.value)}
                    />
                    <span>{option.label}</span>
                  </label>
                ))}
                <label className="chip">
                  <input
                    type="color"
                    aria-label="Custom background colour"
                    onChange={(event) =>
                      setBackground(hexToRgb(event.target.value))
                    }
                  />
                  <span>Custom</span>
                </label>
              </div>
            </fieldset>
          )}

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

        {preflight && preflight.warnings.length > 0 && (
          <div className="field">
            <span className="field__label">Before you convert</span>
            <ul
              className="notice notice--warn"
              aria-label="What this conversion will change"
            >
              {preflight.warnings.map((warning) => (
                <li key={warning.messageKey}>{t(warning.messageKey)}</li>
              ))}
            </ul>
          </div>
        )}

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

      <div className="actionbar">
        <div className="actionbar__inner">
          <p className="actionbar__summary">
            {convertible > 0 ? (
              <>
                <strong>
                  {convertible} image{convertible === 1 ? "" : "s"}
                </strong>{" "}
                → {FORMATS.find((f) => f.value === format)?.label}
                {destination
                  ? ` · saving to ${fileNameOf(destination)}`
                  : " · choose a folder"}
              </>
            ) : (
              "Drop images above, or click to browse."
            )}
          </p>
          {paths.length > 0 && (
            <button
              type="button"
              className="btn btn--quiet"
              onClick={() => setPaths([])}
            >
              Clear
            </button>
          )}
          <button
            type="button"
            className="btn btn--primary"
            disabled={!canRun}
            onClick={() => void run()}
          >
            Convert
            {convertible > 0
              ? ` ${convertible} file${convertible === 1 ? "" : "s"}`
              : ""}
          </button>
        </div>
      </div>
    </div>
  );
}

function hexToRgb(hex: string): Background {
  const value = Number.parseInt(hex.replace("#", ""), 16);
  return {
    r: (value >> 16) & 0xff,
    g: (value >> 8) & 0xff,
    b: value & 0xff,
  };
}
