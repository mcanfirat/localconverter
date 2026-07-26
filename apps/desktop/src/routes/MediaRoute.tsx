import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useState } from "react";

import type { ConversionError } from "../bindings/ConversionError";
import type { MediaFormat } from "../bindings/MediaFormat";
import type { MediaPreset } from "../bindings/MediaPreset";
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

const INPUT_EXTENSIONS = [
  "mp4",
  "mov",
  "mkv",
  "webm",
  "gif",
  "mp3",
  "wav",
  "flac",
  "ogg",
  "m4a",
];

const VIDEO_FORMATS: { value: MediaFormat; label: string }[] = [
  { value: "mp4", label: "MP4" },
  { value: "webm", label: "WebM" },
  { value: "mkv", label: "MKV" },
  { value: "gif", label: "GIF" },
];
const AUDIO_FORMATS: { value: MediaFormat; label: string }[] = [
  { value: "mp3", label: "MP3" },
  { value: "wav", label: "WAV" },
  { value: "flac", label: "FLAC" },
  { value: "ogg", label: "OGG" },
  { value: "m4a", label: "M4A" },
];

const PRESETS: { value: MediaPreset; label: string }[] = [
  { value: "high", label: "High quality" },
  { value: "balanced", label: "Balanced" },
  { value: "small", label: "Small file" },
];

export function MediaRoute() {
  const [available, setAvailable] = useState<boolean | null>(null);
  const [path, setPath] = useState<string | null>(null);
  const [destination, setDestination] = useState<string | null>(null);
  const [format, setFormat] = useState<MediaFormat>("mp4");
  const [preset, setPreset] = useState<MediaPreset>("balanced");
  const [removeAudio, setRemoveAudio] = useState(false);
  const [policy, setPolicy] = useState<OverwritePolicy>("rename");
  const [error, setError] = useState<ConversionError | null>(null);
  const [lastJobId, setLastJobId] = useState<string | null>(null);

  const jobs = useJobStore((state) => state.jobs);
  const lastJob = jobs.find((job) => job.id === lastJobId);

  useEffect(() => {
    void invoke<boolean>("media_available")
      .then(setAvailable)
      .catch(() => setAvailable(false));
  }, []);

  const isVideoTarget = VIDEO_FORMATS.some((f) => f.value === format);

  // One file at a time: the options below (target format, preset, trim) are
  // per-file decisions, and a batch would need them per row.
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
        operationId: "media.convert",
        inputPaths: [path],
        outputDirectory: destination,
        overwritePolicy: policy,
        options: {
          targetFormat: format,
          preset,
          trim: null,
          removeAudio: isVideoTarget ? removeAudio : false,
        },
      });
      setLastJobId(job.id);
    } catch (thrown) {
      setError(toConversionError(thrown));
    }
  }

  if (available === false) {
    return (
      <div className="stack">
        <section className="card card--accent">
          <h2>FFmpeg is not installed</h2>
          <p>{t("error.media.ffmpegMissing")}</p>
          <p className="muted">
            Audio and video conversion use FFmpeg on your own machine — nothing
            is uploaded. LocalConvert does not bundle it yet, so it uses the
            copy you install.
          </p>
        </section>
      </div>
    );
  }

  const canRun = path !== null && destination !== null && available === true;

  return (
    <div className="split">
      <div className="stack">
        <DropZone
          title="Drop a video or audio file here"
          hint="or click to browse — MP4, MOV, MKV, WebM, MP3, WAV, FLAC, OGG, M4A"
          filterName="Audio & video"
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
            <legend className="field__label">Convert to — video</legend>
            <div className="chips">
              {VIDEO_FORMATS.map((option) => (
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
          <fieldset className="field">
            <legend className="field__label">
              Convert to — audio (extract)
            </legend>
            <div className="chips">
              {AUDIO_FORMATS.map((option) => (
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

          <fieldset className="field">
            <legend className="field__label">Quality</legend>
            <div className="chips">
              {PRESETS.map((option) => (
                <label key={option.value} className="chip">
                  <input
                    type="radio"
                    name="preset"
                    checked={preset === option.value}
                    onChange={() => setPreset(option.value)}
                  />
                  <span>{option.label}</span>
                </label>
              ))}
            </div>
          </fieldset>

          {isVideoTarget && format !== "gif" && (
            <label className="field__row">
              <input
                type="checkbox"
                checked={removeAudio}
                onChange={(event) => setRemoveAudio(event.target.checked)}
              />
              <span>Remove audio track</span>
            </label>
          )}
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
            "Drop a file above, or click to browse."
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
