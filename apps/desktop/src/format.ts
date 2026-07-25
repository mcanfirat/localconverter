/** Presentation helpers. Pure, so they are cheap to test. */

const UNITS = ["B", "KiB", "MiB", "GiB", "TiB"] as const;

/**
 * Binary units with explicit KiB/MiB labels rather than 1024-bytes-called-"KB".
 * A conversion tool that reports sizes is the last place to be vague about them.
 */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "—";
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const decimals = unit === 0 || value >= 100 ? 0 : value >= 10 ? 1 : 2;
  return `${value.toFixed(decimals)} ${UNITS[unit]}`;
}

/** Signed, so growth reads as growth. */
export function formatSizeChange(percent: number): string {
  if (!Number.isFinite(percent)) return "—";
  const rounded = Math.round(percent * 10) / 10;
  if (rounded === 0) return "no change";
  return `${rounded > 0 ? "+" : "−"}${Math.abs(rounded).toFixed(1)}%`;
}

export function formatDuration(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return "—";
  if (ms < 1000) return `${Math.round(ms)} ms`;
  const seconds = ms / 1000;
  if (seconds < 60) return `${seconds.toFixed(1)} s`;
  const minutes = Math.floor(seconds / 60);
  return `${minutes} min ${Math.round(seconds % 60)} s`;
}

/** File name only — the UI never renders a full path. */
export function fileNameOf(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] ?? path;
}
