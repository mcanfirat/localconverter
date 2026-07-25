# Privacy

## The promise

> Your files never leave your computer.

Every conversion runs locally. There is no account, no API key, no upload, no
remote conversion service and no external AI service. The application works with
the network cable unplugged.

## How that is enforced, not just asserted

**No network client exists.** There is no HTTP crate in the Rust dependency
tree and no `fetch` in the frontend. Nothing in this build has the capability to
make a request, so the guarantee does not rest on nobody choosing to call it.

**The renderer cannot dial out.** The Content Security Policy in
`tauri.conf.json` restricts `connect-src` to `'self'` and the IPC origin, and
sets `object-src 'none'`, `base-uri 'none'`, `form-action 'none'`. Even injected
script cannot open a connection.

**The frontend cannot read your files.** It holds no filesystem permission. It
can start an operation, cancel one and read job state. File contents never enter
the webview at all — the Rust side reads from disk and writes to disk, and the
UI only ever sees sizes, names and statuses.

## What is stored on your machine

**Logs** — `stderr` plus a daily rolling file in the OS application-log
directory. They contain job ids, operation ids, statuses and error codes.
Detail strings are passed through `error::redact`, which replaces your home
directory path with `<home>`.

**Job history** — this build keeps jobs in memory for the session only. Nothing
is written to a history database. When persistent history lands it will store
metadata only (operation, timestamp, status, display names, size change,
duration), it will be disableable, and it will never store file contents.

**Temporary files** — `<app-temp>/localconvert/jobs/<job-id>/`. Removed when the
job ends, whether it succeeded, failed or was cancelled, and swept again at next
startup if a crash prevented that.

## Telemetry

**None.** No analytics, no crash reporting, no usage counters, no ping.

If anonymous telemetry is ever added it will be opt-in, off by default, free of
filenames, paths, hashes, content and any parameter that could expose private
data, and documented here before it ships.

## Update checks

**None in this build.** If an update check is added it will fetch version
metadata only, be user-controllable, and never include any information about
your files.

## Verifying the claim yourself

- Disconnect from the network and use the application. Nothing degrades.
- `grep -ri "reqwest\|hyper\|ureq\|fetch(" crates apps/desktop/src` — no results.
- Watch it with Little Snitch, `lsof -i`, or Wireshark.
- Read `apps/desktop/src-tauri/capabilities/default.json`: two permissions.
