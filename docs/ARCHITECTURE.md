# Architecture

## The shape of the thing

```
┌─────────────────────────────────────────────┐
│ React + TypeScript  (apps/desktop/src)      │  presentation only
│  · typed IPC wrappers (ipc.ts)              │
│  · job store (zustand)                      │
│  · generated contracts (src/bindings/)      │
└──────────────────┬──────────────────────────┘
                   │  Tauri IPC — 8 named commands, nothing generic
┌──────────────────▼──────────────────────────┐
│ Desktop shell  (apps/desktop/src-tauri)     │
│  · commands.rs — validates every argument   │
│  · state.rs    — registry, scheduler, events│
└──────────────────┬──────────────────────────┘
                   │  plain Rust calls, no Tauri types
┌──────────────────▼──────────────────────────┐
│ localconvert-core  (crates/)                │
│  job · paths · workspace · validation       │
│  detect · image_engine · runner · operation  │
└─────────────────────────────────────────────┘
```

The core crate has no Tauri dependency. That is what lets the same pipeline back
the desktop app, the future CLI, and the integration tests — and it is why the
interesting logic is testable without a window.

## The three-stage pipeline

Every job, from the Phase 0 self-test to the video encoder that lands in Phase 5,
runs the same three stages:

```
execute()   plugin writes staged output into <app-temp>/jobs/<job-id>/
validate()  plugin reads it back and reports on it
commit()    shared code moves it to the user's folder — only now
```

Plugins own `execute` and `validate`. **`commit` is shared and always runs
last.** That is the structural reason "nothing unvalidated reaches the user's
folder" is a property of the pipeline rather than a promise each plugin must
individually keep. A plugin author cannot forget it, because they never write it.

`commit` refuses outright if any report is invalid, deletes the staged file, and
reports `partialOutputRemoved: true` so the UI can tell the user exactly what
happened to it.

## The state machine

```
Queued → Preparing → Running → Validating → Completed
                                          → CompletedWithWarnings

any non-terminal → Failed | Cancelled
```

`Completed` is reachable **only** from `Validating`. There is no other edge into
it. `ConversionJob::transition_to` is the only function that writes
`job.status`, and it rejects anything the table above does not permit.

Warnings, not the plugin, decide between `Completed` and
`CompletedWithWarnings`: `JobResult::warnings` being non-empty is the whole
condition.

## Progress is honest

`JobProgress::percent` is `Option<f32>`. It is `None` whenever the underlying
work cannot be counted, and the UI renders an indeterminate bar plus the stage
name. `JobProgress::counted(done, total)` returns `None` for a zero total rather
than fabricating 0% or 100%.

There is no interpolation, no time-based estimate, and no synthetic animation
standing in for real progress anywhere in the codebase.

## Format detection

`core::detect` reads a file's leading bytes and identifies it: magic signatures
first, then container inspection (RIFF sub-type for WebP vs WAV, ISO-BMFF brands
for AVIF vs HEIC vs MP4), and only then the extension — at 0.3 confidence, and
flagged as `extension_mismatch` when the two disagree.

Nothing branches on the extension alone. A `.jpg` holding PNG bytes is converted
as a PNG and the user is told.

## Safety primitives

Everything filesystem-shaped routes through `core::paths`:

- `ensure_safe_component_bytes` — rejects NUL bytes before any path reaches an OS
  API or a future child-process argv.
- `canonicalize_deepest_existing` — resolves as much of a path as exists, so
  containment can be checked on paths we are about to *create*. `..` in the
  not-yet-existing remainder is rejected rather than normalised lexically,
  because lexical normalisation is wrong once a symlink appears mid-flight.
- `is_within` / `join_within` — canonical containment. Used today by the temp
  workspace; it is the same primitive archive extraction will use in Phase 2.
- `resolve_destination` — applies the overwrite policy, producing
  `photo (1).jpg` for `Rename`.
- `commit_output` — rename, falling back to copy-then-remove across volumes.

`core::workspace` fences every deletion: a directory is removed only if its name
parses as a UUID *and* it canonically resolves inside `<app-temp>/jobs`. A
symlink planted under that directory resolves outside and is therefore skipped,
which is asserted by a test.

## Concurrency

One job at a time in Phase 0 (`MAX_CONCURRENT_JOBS`). Jobs beyond that wait on a
semaphore permit, and a cancel issued while queued is honoured immediately
rather than after the permit arrives. Per-category limits (heavy media 1, images
scaled to CPU) arrive with the engines that need them.

## IPC surface

Eight commands, all in `commands.rs`:

`app_info`, `list_operations`, `preflight_images`, `list_jobs`, `get_job`,
`start_job`, `cancel_job`, `clear_completed_jobs`

There is no `read_file`, no `write_file`, no `exec`, and no passthrough. The
window capability grants exactly `core:event:default` and `dialog:allow-open` —
no `fs:`, `shell:`, `process:`, `http:` or `opener:` permission. Both facts are
asserted by tests in `commands.rs`, so a future passthrough trips CI.

## Contract generation

Rust structs carry `#[derive(TS)] #[ts(export)]`. `cargo test` writes the
TypeScript mirror into `apps/desktop/src/bindings/`, and CI fails on a dirty
tree afterwards. The frontend cannot drift from the backend because it does not
own its own copy of the types.

`u64` fields are annotated `#[ts(type = "number")]`: Tauri serialises over JSON,
so they arrive in JavaScript as plain numbers. ts-rs would otherwise emit
`bigint`, which is not what the bridge actually delivers.

## Events

One event, `job-updated`, carrying the whole `ConversionJob`. The frontend
replaces its copy by id. A single channel rather than separate
progress/status/completion events, so the UI can never render a stale status
beside fresh progress.
