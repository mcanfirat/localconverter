# Contributing

## Before anything else

```bash
pnpm install
pnpm check     # typecheck, lint, vitest, cargo fmt, clippy, cargo test
```

If `pnpm check` is green you are ready. If it is not, that is the first thing to
fix, whatever else you were planning.

## The rules that are not negotiable

These come from the specification and are why the project exists. A pull request
that weakens one will not be merged, however convenient it is.

1. **A source file is never modified.** Inputs are opened read-only.
2. **No output is called successful without validation.** If you find yourself
   wanting a shortcut past `Validating`, the design is wrong, not the state
   machine.
3. **No shell command strings.** Argument arrays only. No `sh -c`.
4. **Nothing is uploaded, ever.** Adding an HTTP client is a design change that
   needs discussion first, not a dependency bump.
5. **Fail clearly rather than guess.** An unsupported operation is an error. A
   silent fallback — a changed format, a dropped alpha channel, a reduced
   resolution — is a data-loss bug.
6. **Do not weaken a test to make CI pass.** If a test is wrong, fix the test on
   its own merits and say why in the commit.

## Scope

Work on **one phase or one issue at a time**. Do not implement images, PDF and
media in one branch. [docs/ROADMAP.md](docs/ROADMAP.md) has the order, and the
order is deliberate: breadth before depth is how conversion tools end up with
forty buttons and no reliability.

Do not pre-create empty crates or workspace packages for future phases. When the
phase arrives it will bring its own crate.

## Adding a conversion route

1. Implement `execute` (write staged output into the job's temp workspace) and
   `validate` (read it back, produce a `ValidationReport`). **Do not write to the
   destination** — `runner::commit` does that, after validation, for everything.
2. Build the report with `ValidationReport::from_checks` so `valid` is derived
   rather than asserted. Start from `validation::basic_output_checks` and append
   your format-specific checks.
3. Register the operation in `operation::list_operations` **only once its
   fixtures pass**. The registry is what the UI renders; an entry there is a
   promise.
4. Add fixtures: valid small, valid large, corrupted, wrong-extension,
   metadata-heavy, Unicode filename, filename with spaces. Plus read-only
   destination, cancellation and overwrite-conflict tests.
5. Report progress honestly. If the engine cannot be counted, use
   `JobProgress::indeterminate`. Do not interpolate.
6. Check cancellation inside every loop, and remove partial output when it fires.
7. Update [docs/CONVERSION_MATRIX.md](docs/CONVERSION_MATRIX.md) with the status
   you can actually defend, per platform.

## Code style

**Rust** — rustfmt, and clippy with warnings as errors. `unwrap`, `expect`,
`panic` and raw indexing are denied in production code and allowed inside
`mod tests`. Every error maps to a `ConversionError` with a code and a message
key; no `io::Error` escapes the core crate.

**TypeScript** — strict mode, no `any` (lint error, not a warning). Import
contract types from `src/bindings/` — never hand-write a type that mirrors a
Rust struct. Only `ipc.ts` calls `invoke`.

**Generated code** — `apps/desktop/src/bindings/` is written by ts-rs during
`cargo test`. Never edit it; commit what the tests produce.

**Message strings** — Rust sends a `messageKey`, never prose. Add the string to
`apps/desktop/src/messages.ts`; a test fails if a key has no string.

## Deliberate simplifications

Shortcuts are marked `// ponytail:` with the ceiling and the upgrade path:

```rust
// ponytail: TOCTOU window between exists() and rename(). On a single-user
// desktop the loser is the user's own second job, and the queue serialises
// those. Upgrade to O_EXCL create + rename if a headless mode ever exists.
```

Grep for them before assuming something was forgotten. If you hit a marked
ceiling, that comment is your brief.

## Pull requests

- One issue per PR.
- Tests with the change, in the same commit.
- Say which commands you ran and what they printed.
- Update the docs that your change makes wrong — especially the conversion
  matrix, which must never claim more than CI has verified.
- If something is incomplete, say so in the PR rather than leaving it to be
  discovered.

## Code of conduct

Be decent. Assume good faith. Disagree about the code, not the person.
