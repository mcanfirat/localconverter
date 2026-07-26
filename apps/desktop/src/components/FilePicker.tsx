//! The three pieces every conversion route is built from: a drop target, the
//! list of what was picked, and a pinned bar carrying the primary action.
//!
//! They live here rather than in each route because all five routes are the
//! same shape — choose files, set options, run — and five copies of the
//! drag-drop wiring would drift apart the first time one of them was fixed.

import { open } from "@tauri-apps/plugin-dialog";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { useCallback, useEffect, useState, type ReactNode } from "react";

/**
 * Files dropped on the window arrive as a webview event carrying real paths.
 * The renderer never sees a path from an HTML drop, so the DOM drag events
 * cannot be used for this.
 *
 * `getCurrentWebview` throws outright when the Tauri runtime is absent — as it
 * is when the frontend is opened in an ordinary browser during development —
 * so the subscription is guarded. Losing drag-and-drop there is a nuisance;
 * taking the route down with it was a blank screen.
 */
export function useFileDrop(
  extensions: string[],
  onAdd: (paths: string[]) => void,
): boolean {
  const [dragging, setDragging] = useState(false);

  // Depended on by value, not by identity: routes build the array inline, so a
  // fresh reference every render would tear down and re-add the listener every
  // render. A plain string is stable when the contents are.
  const allowed = extensions.join(",");

  const accept = useCallback(
    (incoming: string[]) => {
      // An empty list means "anything goes" — creating an archive takes any
      // file. Filtering on an empty array would instead drop every drop.
      const wanted =
        allowed === ""
          ? incoming
          : incoming.filter((path) =>
              allowed
                .split(",")
                .includes(path.split(".").pop()?.toLowerCase() ?? ""),
            );
      if (wanted.length > 0) onAdd(wanted);
    },
    [allowed, onAdd],
  );

  useEffect(() => {
    let stop: (() => void) | undefined;
    let cancelled = false;
    try {
      void getCurrentWebview()
        .onDragDropEvent((event) => {
          if (event.payload.type === "over") setDragging(true);
          else if (event.payload.type === "drop") {
            setDragging(false);
            accept(event.payload.paths);
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
  }, [accept]);

  return dragging;
}

export function DropZone({
  title,
  hint,
  filterName,
  extensions,
  multiple = true,
  empty,
  dragging,
  onAdd,
}: {
  title: string;
  hint: string;
  filterName: string;
  extensions: string[];
  multiple?: boolean;
  empty: boolean;
  dragging: boolean;
  onAdd: (paths: string[]) => void;
}) {
  async function browse() {
    // No extensions means every file is a candidate, so the dialog gets no
    // filter at all. An empty `extensions` array would show a filter that
    // matches nothing. `exactOptionalPropertyTypes` forbids passing
    // `filters: undefined`, hence the spread rather than a ternary value.
    const picked = await open({
      multiple,
      ...(extensions.length > 0
        ? { filters: [{ name: filterName, extensions }] }
        : {}),
    });
    if (Array.isArray(picked)) onAdd(picked);
    else if (typeof picked === "string") onAdd([picked]);
  }

  return (
    <button
      type="button"
      className={[
        "dropzone",
        dragging ? "dropzone--hot" : "",
        empty ? "dropzone--empty" : "",
      ]
        .filter(Boolean)
        .join(" ")}
      onClick={() => void browse()}
    >
      <span className="dropzone__icon" aria-hidden="true">
        ⬒
      </span>
      <span className="dropzone__title">
        {dragging ? "Release to add" : title}
      </span>
      <span className="dropzone__hint">{hint}</span>
    </button>
  );
}

// `undefined` is spelled out rather than left to `?`, because the project
// builds with exactOptionalPropertyTypes: callers derive these from data and
// need to pass `undefined` for "no tone" without omitting the key entirely.
export type PickedRow = {
  name: string;
  meta?: ReactNode | undefined;
  tone?: "warn" | "bad" | undefined;
};

export function PickedFiles({
  rows,
  noun,
  total,
  onRemove,
}: {
  rows: PickedRow[];
  noun: string;
  total?: string | undefined;
  onRemove?: ((index: number) => void) | undefined;
}) {
  if (rows.length === 0) return null;
  return (
    <>
      <div className="row row--between">
        <strong>
          {rows.length} {noun}
          {rows.length === 1 ? "" : "s"}
        </strong>
        {total && <span className="muted">{total}</span>}
      </div>
      <ul className="filelist">
        {rows.map((row, index) => (
          <li
            key={`${row.name}-${index}`}
            className={
              row.tone
                ? `filelist__row filelist__row--${row.tone}`
                : "filelist__row"
            }
          >
            <span className="filelist__thumb" aria-hidden="true">
              {row.tone === "bad" ? "✕" : row.tone === "warn" ? "⚠" : "▣"}
            </span>
            <span className="filelist__text">
              <span className="filelist__name">{row.name}</span>
              {row.meta && <span className="filelist__meta">{row.meta}</span>}
            </span>
            {onRemove && (
              <button
                type="button"
                className="filelist__drop"
                aria-label={`Remove ${row.name}`}
                onClick={() => onRemove(index)}
              >
                ✕
              </button>
            )}
          </li>
        ))}
      </ul>
    </>
  );
}

export function ActionBar({
  summary,
  onClear,
  onRun,
  runLabel,
  canRun,
}: {
  summary: ReactNode;
  onClear?: () => void;
  onRun: () => void;
  runLabel: string;
  canRun: boolean;
}) {
  return (
    <div className="actionbar">
      <div className="actionbar__inner">
        <p className="actionbar__summary">{summary}</p>
        {onClear && (
          <button type="button" className="btn btn--quiet" onClick={onClear}>
            Clear
          </button>
        )}
        <button
          type="button"
          className="btn btn--primary"
          disabled={!canRun}
          onClick={onRun}
        >
          {runLabel}
        </button>
      </div>
    </div>
  );
}
