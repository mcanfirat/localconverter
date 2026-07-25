import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";

import type { ConversionError } from "../bindings/ConversionError";
import type { MediaFormat } from "../bindings/MediaFormat";
import type { MediaPreset } from "../bindings/MediaPreset";
import type { OverwritePolicy } from "../bindings/OverwritePolicy";
import { JobCard } from "../components/JobCard";
import { fileNameOf } from "../format";
import { startJob, toConversionError } from "../ipc";
import { t } from "../messages";
import { useJobStore } from "../store";

const INPUT_EXTENSIONS = ["mp4", "mov", "mkv", "webm", "gif", "mp3", "wav", "flac", "ogg", "m4a"];

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
    void invoke<boolean>("media_available").then(setAvailable).catch(() => setAvailable(false));
  }, []);

  const isVideoTarget = VIDEO_FORMATS.some((f) => f.value === format);

  async function chooseFile() {
    const picked = await open({
      multiple: false,
      filters: [{ name: "Audio & video", extensions: INPUT_EXTENSIONS }],
    });
    if (typeof picked === "string") setPath(picked);
  }

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
            is uploaded. LocalConvert does not bundle it yet, so it uses the copy
            you install.
          </p>
        </section>
      </div>
    );
  }

  const canRun = path !== null && destination !== null && available === true;

  return (
    <div className="stack">
      <section className="card">
        <h2>{t("operation.media.label")}</h2>
        <p className="muted">{t("operation.media.description")}</p>

        <div className="field__row">
          <button type="button" className="btn" onClick={() => void chooseFile()}>
            Choose a file…
          </button>
          <button type="button" className="btn" onClick={() => void chooseDestination()}>
            Choose destination…
          </button>
          <span className="muted">
            {path ? fileNameOf(path) : "No file chosen"}
            {destination ? ` → ${fileNameOf(destination)}` : ""}
          </span>
        </div>
      </section>

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
          <legend className="field__label">Convert to — audio (extract)</legend>
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

      <div className="field__row">
        <button
          type="button"
          className="btn btn--primary"
          disabled={!canRun}
          onClick={() => void run()}
        >
          Convert
        </button>
        {available === null && <span className="muted">Checking for FFmpeg…</span>}
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
