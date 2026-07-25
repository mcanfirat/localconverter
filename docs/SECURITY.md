# Security model

## Threat model

LocalConvert's job is to take a file the user obtained from somewhere — an
email attachment, a download, a USB stick — and run it through a parser. That is
inherently an attack surface, and the design assumes every input is hostile.

| Threat | Status | Mitigation |
|---|---|---|
| Malicious filenames (NUL bytes, traversal, absolute paths) | **Mitigated** | `paths::ensure_safe_component_bytes`, `join_within`. Tested. |
| Path traversal into or out of the temp root | **Mitigated** | Canonical containment via `is_within`. Tested, including symlink escape. |
| Temp cleanup deleting user data | **Mitigated** | `remove_fenced` + UUID-named-entries-only rule. Tested with a symlink disguised as a job directory. |
| Overwriting the user's source file | **Mitigated** | `validation::basic_output_checks` compares canonicalised input and output paths; `commit_output` refuses to overwrite without an explicit policy. |
| Frontend reaching the filesystem directly | **Mitigated** | Capability grants no `fs:`/`shell:`/`process:` permission; asserted by test. Eight named commands, no passthrough. |
| Renderer exfiltrating data | **Mitigated** | CSP restricts `connect-src` to `self` and the IPC origin. No HTTP client in the dependency tree. |
| Command injection | **Mitigated** | The media engine passes arguments as an array to FFmpeg — no shell string is ever built. Paths are NUL-checked and canonicalised first. |
| Archive bombs, symlink escape on extract | **Not applicable yet** | Phase 2. The containment primitive it will use already exists and is tested. |
| Malformed media / PDF crashing a parser | **Not applicable yet** | Phases 3 and 5. Engines will run as sandboxed child processes so a parser crash cannot take the app with it. |
| Compromised bundled binary | **Not applicable yet** | Phase 5. Pinned versions + SHA-256 verified at download, at packaging, and before execution. |
| Untrusted plugins | **Deferred** | Phase 8. Third-party binary plugins are not dynamically loadable until a signing and permission model exists. |

Rows marked *not applicable yet* are honest: the code that would need the
mitigation does not exist in this build. They are listed so that adding that code
without its mitigation is visibly incomplete.

## Rules for child processes

Enforced by `crate::media`'s runner for the system FFmpeg, and required of any
future child process:

- Never construct a shell command string. Never `sh -c`, `cmd /C`, or PowerShell
  concatenation.
- Pass arguments as an array.
- Reject NUL bytes (`ensure_safe_component_bytes` already does).
- Canonicalise input and output paths before passing them.
- Run inside the job's own temp directory, owned by the application.
- Enforce a timeout.
- Limit inherited environment variables.
- Capture stdout and stderr; redact paths before they reach a diagnostics bundle.
- Terminate the whole process tree on cancellation.
- Verify the binary's checksum before executing it.
- No console window on Windows.

## IPC surface

```
app_info · list_operations · preflight_images · list_jobs
get_job · start_job · cancel_job · clear_completed_jobs
```

`start_job` and `preflight_images` are the only commands that accept paths, and
`preflight_images` only ever reads.

`start_job` validates that the operation exists, that the destination is an
existing directory, that the input count matches what the operation accepts, and
that every input is a regular file it can stat — before a job id is ever created.

Window capability (`apps/desktop/src-tauri/capabilities/default.json`):

```json
"permissions": ["core:event:default", "dialog:allow-open"]
```

## Diagnostics redaction

`ConversionError::new` runs every detail string through `error::redact`, which
replaces the user's home directory prefix with `<home>`. `display_name` returns
the file name only. The UI never renders a full path — a screenshot of a failed
job cannot leak the user's directory structure.

## Reporting a vulnerability

See [`/SECURITY.md`](../SECURITY.md) in the repository root.
