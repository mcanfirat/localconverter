/**
 * The entire backend surface, typed against the ts-rs bindings generated from
 * the Rust contracts. Nothing else in the frontend calls `invoke` directly.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type { AppInfo } from "./bindings/AppInfo";
import type { ConversionError } from "./bindings/ConversionError";
import type { ConversionJob } from "./bindings/ConversionJob";
import type { ImageBatchPreflight } from "./bindings/ImageBatchPreflight";
import type { OperationDescriptor } from "./bindings/OperationDescriptor";
import type { StartJobRequest } from "./bindings/StartJobRequest";

export const JOB_UPDATED_EVENT = "job-updated";

export function appInfo(): Promise<AppInfo> {
  return invoke<AppInfo>("app_info");
}

export function listOperations(): Promise<OperationDescriptor[]> {
  return invoke<OperationDescriptor[]>("list_operations");
}

/** Inspects a selection without converting it, so warnings precede the button. */
export function preflightImages(
  inputPaths: string[],
  options: unknown,
): Promise<ImageBatchPreflight> {
  return invoke<ImageBatchPreflight>("preflight_images", { inputPaths, options });
}

export function listJobs(): Promise<ConversionJob[]> {
  return invoke<ConversionJob[]>("list_jobs");
}

export function startJob(request: StartJobRequest): Promise<ConversionJob> {
  return invoke<ConversionJob>("start_job", { request });
}

export function cancelJob(jobId: string): Promise<void> {
  return invoke<void>("cancel_job", { jobId });
}

export function clearCompletedJobs(): Promise<ConversionJob[]> {
  return invoke<ConversionJob[]>("clear_completed_jobs");
}

export function onJobUpdated(
  handler: (job: ConversionJob) => void,
): Promise<UnlistenFn> {
  return listen<ConversionJob>(JOB_UPDATED_EVENT, (event) =>
    handler(event.payload),
  );
}

/**
 * Commands reject with the serialized `ConversionError`. Anything else is a bug
 * in the bridge rather than a conversion failure, so it is reported as one
 * instead of being silently swallowed.
 */
export function toConversionError(thrown: unknown): ConversionError {
  if (isConversionError(thrown)) return thrown;
  return {
    code: "internalError",
    messageKey: "error.internalError",
    detail: typeof thrown === "string" ? thrown : String(thrown),
    sourceSafe: true,
    partialOutputRemoved: false,
  };
}

function isConversionError(value: unknown): value is ConversionError {
  return (
    typeof value === "object" &&
    value !== null &&
    "code" in value &&
    "messageKey" in value &&
    typeof (value as { messageKey: unknown }).messageKey === "string"
  );
}
