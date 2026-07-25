# LocalConvert

**Private file conversion and compression for Windows, macOS, and Linux.**

**Your files never leave your computer.**

---

## Install — one command

Paste this into a terminal. Nothing else to download, clone or click.

**macOS / Linux**
```bash
curl -fsSL https://raw.githubusercontent.com/mcanfirat/localconverter/main/scripts/bootstrap.sh | bash
```

**Windows** (PowerShell)
```powershell
irm https://raw.githubusercontent.com/mcanfirat/localconverter/main/scripts/bootstrap.ps1 | iex
```

It clones the repo into `~/localconvert`, installs the tools it needs (Rust,
Node, pnpm, FFmpeg), builds LocalConvert and **opens the app**. Run it again any
time — it pulls the latest code and rebuilds.

Already have the folder? `bash scripts/setup.sh` does everything except the clone.

| | |
|---|---|
| **Time** | 5–15 minutes the first time (it compiles from source). Minutes after. |
| **Disk** | About **5 GB**: ~1.5 GB toolchain, ~2.5 GB build files, ~200 MB packages. |
| **Internet** | Needed **only** for this install. Converting files never uses the network. |
| **Safe to re-run** | Yes. Skips anything already installed, touches none of your files. |

Per-system notes and troubleshooting: [macOS](#macos) · [Linux](#linux) ·
[Windows](#windows).

---

> ### What works today
>
> **Images** (JPG/PNG/WebP/TIFF/BMP/GIF, resize, quality, batch) · **Archives**
> (ZIP/TAR/TAR.GZ create & safe extract) · **PDF** (merge, split, extract/reorder/
> rotate pages, images→PDF, strip metadata) · **Spreadsheets** (CSV/TSV/XLSX/JSON
> with value preservation) · **Audio/Video** (via a system-installed FFmpeg) ·
> a **CLI** covering all of it. Desktop app + CLI, macOS verified.
>
> **Needs a native component, so declined by name (never faked):** HEIC, AVIF and
> lossy WebP (image codecs); PDF→images (a rasteriser); a *bundled* FFmpeg (media
> works today against a system FFmpeg). See
> [the matrix](docs/CONVERSION_MATRIX.md) for exactly what is verified per platform.

## Why this exists

Most "convert your file" tools upload your document to somebody else's server.
For a holiday photo that's a shrug; for a payslip, a passport scan, a contract
or a medical record it is a data transfer you did not intend to make.

LocalConvert does the work on your own machine. It also refuses to lie to you
about the result: an output is verified before it is saved, and anything that
fails verification is deleted rather than handed over looking successful.

## Guarantees

These are enforced by tests, not by good intentions — the enforcing test is named
in [CLAUDE.md](CLAUDE.md).

- **Nothing is uploaded.** No HTTP client exists anywhere in the dependency tree.
  The renderer's CSP blocks outbound connections. It works offline.
- **Your originals are never modified.** Inputs are opened read-only, and a job
  whose output resolves to one of its own inputs fails validation.
- **No result is called successful until it is verified.** The state machine
  admits `Completed` only from `Validating`.
- **Nothing unverified reaches your folder.** Output is staged in a temp
  workspace and moved to the destination only after its checks pass.
- **No file is overwritten silently.** You choose stop, rename, skip or replace.
- **Progress is real.** When work cannot be counted the bar is indeterminate
  rather than a fake animation.

## Supported operating systems

| | Status |
|---|---|
| macOS 13+ (Apple Silicon) | ✅ builds and runs, release bundle verified |
| macOS 13+ (Intel) | ⏳ builds in CI, not yet run on hardware |
| Windows 10/11 x64 | ⏳ builds in CI, not yet run on hardware |
| Linux x64 (Ubuntu/Debian) | ⏳ builds in CI, not yet run on hardware |

## Supported conversions

**Images** — read JPG/PNG/WebP/TIFF/BMP/GIF, write JPG/PNG/WebP/TIFF/BMP; batch,
resize, JPEG quality, EXIF orientation baked in, metadata stripping.
**Archives** — create and safely extract ZIP/TAR/TAR.GZ (traversal, symlink and
bomb protection). **PDF** — merge, split, extract/reorder/rotate pages,
images→PDF, remove metadata. **Spreadsheets** — CSV/TSV/XLSX/JSON with exact
value preservation. **Media** — video/audio conversion, extract audio, trim,
MP4→GIF via a system FFmpeg. Everything is scriptable through the
[CLI](docs/CLI.md).

Three things it does that most converters do not:

- **Refuses to flatten transparency behind your back.** Converting a
  transparent PNG to JPG stops and asks for a background colour.
- **Trusts the bytes, not the extension.** A PNG named `.jpg` is converted as a
  PNG, and you are told about the mismatch.
- **Re-opens every result before saving it.** Magic bytes, an independent
  header parser and a full decode all have to agree, or the job fails and the
  output is deleted.

Not yet: HEIC, AVIF, lossy WebP (all need native codecs), crop/rotate,
target-size mode, ICC colour management, metadata preservation. Full status per
platform: [docs/CONVERSION_MATRIX.md](docs/CONVERSION_MATRIX.md).

## Install & run

The [one command](#install--one-command) at the top covers every system. What
follows is what it does on yours, and what to do if a step fails.

Options — append them to either the bootstrap command or `scripts/setup.sh`:

```bash
dev          # just launch the app with hot reload, don't build an installer
--no-media   # skip FFmpeg (audio and video stay disabled)
```

And to control where it lands:

```bash
LOCALCONVERT_DIR=~/apps/localconvert   # clone target, default ~/localconvert
LOCALCONVERT_REPO=https://…/fork.git   # clone a fork instead
```

> Piping a script from the internet into a shell runs whatever is at that URL.
> That is how `rustup` and Homebrew install too, but if you would rather look
> first: download the file, read it, then run it — it is about 45 lines.

---

### macOS

**You need:** macOS 13 (Ventura) or newer, Apple Silicon or Intel.

The command installs, in order:
1. Apple's **Command Line Tools** (which include `git`). If a system dialog
   opens, click **Install**, wait, then run the command again.
2. The **Rust** toolchain, via [rustup](https://rustup.rs).
3. **Node.js** and **pnpm**, via [Homebrew](https://brew.sh) — if you have
   neither Homebrew nor Node, it tells you to install Homebrew first.
4. **FFmpeg** (audio and video only).
5. Then it builds the app and **opens it**.

**Keeping the app:** Finder → `~/localconvert/target/release/bundle/macos/` →
drag **LocalConvert.app** into **Applications**. A shareable `.dmg` sits in
`target/release/bundle/dmg/`.

> **If macOS says "LocalConvert can't be opened" or "is damaged":** the build is
> not Apple-notarized (that needs a paid Apple Developer certificate). It is not
> broken. **Right-click the app → Open → Open.** You only do this once. The setup
> script already clears the flag on the copy it builds, so usually it just opens.

---

### Linux

**You need:** a 64-bit desktop distribution with GTK — Ubuntu, Debian, Fedora,
Arch and openSUSE are all fine — plus `git` and `curl` (`sudo apt install git curl`).

It will ask for your password once, because the desktop libraries need `sudo`.
On **Ubuntu/Debian** it installs:

```
libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev
librsvg2-dev patchelf build-essential curl file
```

On **Fedora**:

```
webkit2gtk4.1-devel gtk3-devel libappindicator-gtk3-devel librsvg2-devel patchelf
```

On **Arch** or **openSUSE** the script installs Rust, Node and FFmpeg but prints a
warning about the GUI libraries — install them yourself first:

```bash
# Arch
sudo pacman -S webkit2gtk-4.1 gtk3 libayatana-appindicator librsvg patchelf base-devel
# openSUSE
sudo zypper install webkit2gtk3-devel gtk3-devel libappindicator3-devel librsvg-devel patchelf
```

**Keeping the app:** bundles land in `~/localconvert/target/release/bundle/`:

```bash
# AppImage — portable, no install
chmod +x target/release/bundle/appimage/*.AppImage
./target/release/bundle/appimage/*.AppImage

# or the Debian package
sudo apt install ./target/release/bundle/deb/*.deb
```

> If the window opens blank or the build complains about `webkit2gtk`, the GTK
> development packages above are the missing piece.

---

### Windows

**You need:** Windows 10 or 11, 64-bit, and **git** (`winget install --id Git.Git -e`).

Open PowerShell from the Start menu and paste the
[one command](#install--one-command). It installs, via `winget`:
1. **Microsoft C++ Build Tools** — Rust links with MSVC, so without these the
   build stops at `linker 'link.exe' not found`. This is the big download.
2. **WebView2 runtime** — how the window draws. Built into Windows 11; some
   Windows 10 machines need it.
3. **Rust**, **Node.js LTS**, **pnpm**, and **FFmpeg** (audio/video only).

> If `winget` is missing, install **App Installer** from the Microsoft Store, or
> install [Rust](https://rustup.rs), [Node](https://nodejs.org) and the
> [C++ Build Tools](https://visualstudio.microsoft.com/downloads/) (choose the
> **Desktop development with C++** workload) by hand, then re-run.

> **If the build stops right after installing the tools:** close PowerShell,
> reopen it, and run the command again. Installers add themselves to `PATH`, and
> an already-open terminal will not see them.

**Keeping the app:** `~\localconvert\target\release\bundle\` contains an
**`.msi`** and an **NSIS `.exe`** — run either to install LocalConvert normally.

> Windows may warn that the publisher is unknown: the build is not code-signed
> (that needs a paid certificate). Choose **More info → Run anyway**.

---

### Prefer the terminal to a window?

Every feature is also a command-line tool — no GUI needed:

```bash
cargo run --release -p localconvert-cli -- convert photo.png --to jpg
cargo run --release -p localconvert-cli -- --help
```

Full usage and exit codes: [docs/CLI.md](docs/CLI.md).

### If something goes wrong

| What you see | What it means | Fix |
|---|---|---|
| `git is required` | The command needs git to clone | macOS: `xcode-select --install` · Linux: `sudo apt install git` · Windows: `winget install --id Git.Git -e` |
| `could not clone …` | Wrong address, or no connection | Check the URL is the real repo, not `mcanfirat/localconverter` |
| `… already exists and is not a git checkout` | Something else is at `~/localconvert` | Move it, or set `LOCALCONVERT_DIR` elsewhere |
| A dialog about **Command Line Tools** (macOS) | Apple's compilers are installing | Finish the dialog, then run the command again |
| `Homebrew isn't installed` (macOS) | Node/FFmpeg need it | Install [Homebrew](https://brew.sh), re-run |
| *"LocalConvert can't be opened"* (macOS) | Not notarized — not broken | Right-click the app → **Open** → **Open** |
| `linker 'link.exe' not found` (Windows) | C++ Build Tools missing | Let the script install them, reopen PowerShell, re-run |
| `cargo: command not found` after installing | Terminal has a stale `PATH` | Close and reopen the terminal, re-run |
| Blank window (Linux) | GTK/WebKit dev packages missing | Install the packages listed under Linux above |
| Media tab says FFmpeg is missing | FFmpeg not on `PATH` | `brew install ffmpeg` / `sudo apt install ffmpeg`, reopen the app. Everything except audio/video works without it |
| Build stops with red errors | Something above is missing | Copy the **last ~15 lines** — they name the missing piece |

### Doing it by hand

If you would rather not run a script: install **Rust ≥ 1.77**, **Node ≥ 20**,
**pnpm 10**, your platform's GUI dev packages (above), and optionally **FFmpeg**.
Then:

```bash
pnpm install
pnpm tauri build
```

Details in [docs/PACKAGING.md](docs/PACKAGING.md).

### Running your own fork

Point the bootstrap at it — no need to edit any script:

```bash
LOCALCONVERT_REPO=https://github.com/you/localconverter.git \
  bash scripts/bootstrap.sh
```

To change the default permanently, edit `DEFAULT_REPO` in `scripts/bootstrap.sh`
and `$DefaultRepo` in `scripts/bootstrap.ps1`.

### Uninstalling

Delete the project folder (this removes all build files), and drag
LocalConvert.app to the Trash / uninstall it from Windows Settings. The tools it
installed — Rust, Node, FFmpeg — are ordinary developer tools and stay unless you
remove them yourself (`rustup self uninstall`, `brew uninstall ffmpeg`, …).

> **Tested on macOS (Apple Silicon) only.** The Linux and Windows steps are
> written against each platform's documented requirements and their CI is
> defined, but they have not yet been run on real hardware. If you hit a problem
> there, the error text is genuinely useful — please report it.

## Developing

```bash
pnpm dev      # run the app with hot reload
pnpm check    # everything CI runs: typecheck, lint, tests, fmt, clippy
```

`cargo test` regenerates the TypeScript contracts in
`apps/desktop/src/bindings/` from the Rust types via ts-rs. Commit what it
produces; CI fails if they drift.

## Security model

Eight named IPC commands. No generic filesystem, shell or process access is
exposed to the frontend — the window capability grants exactly two permissions,
and a test fails the build if that changes. Details and the full threat model:
[docs/SECURITY.md](docs/SECURITY.md).

Report vulnerabilities per [SECURITY.md](SECURITY.md).

## How verification works

A conversion is not "the command exited 0". Before an output is saved,
LocalConvert checks that the file exists, is non-empty, is not one of the
inputs, has the expected name and extension, and — per format — that an
independent parser can read it back with the expected properties. Every check is
recorded in a `ValidationReport` whose `valid` flag is *derived* from the checks,
so a plugin cannot report success beside a failing check.

If verification fails, the job fails, the invalid output is deleted, your source
is untouched, and the UI says which check failed.

## Documentation

| | |
|---|---|
| [ARCHITECTURE.md](docs/ARCHITECTURE.md) | How the pipeline is put together and why |
| [CONVERSION_MATRIX.md](docs/CONVERSION_MATRIX.md) | What is actually supported, per platform |
| [ROADMAP.md](docs/ROADMAP.md) | Phases, in order, with acceptance criteria |
| [TESTING.md](docs/TESTING.md) | What is tested, what is not yet, and when it arrives |
| [SECURITY.md](docs/SECURITY.md) | Threat model and mitigations |
| [PRIVACY.md](docs/PRIVACY.md) | The privacy claim and how to verify it yourself |
| [PACKAGING.md](docs/PACKAGING.md) | Building, signing, bundling |
| [CLI.md](docs/CLI.md) | Command-line usage and exit codes |

## Known limitations

- HEIC, AVIF and lossy WebP output need native codecs that are not bundled.
- PDF→images and in-PDF image recompression need a rasteriser (not bundled).
- Media needs a system-installed FFmpeg; none is bundled yet.
- Metadata is always stripped; preserving it is not implemented (you are warned
  when a file had any).
- Animated GIF and WebP export the first frame only, with a warning.
- No ICC colour management — profiles are not read or converted.
- Job history is in-memory for the session and is not persisted.
- One job runs at a time.
- macOS builds are unsigned and un-notarized; Windows installers are unsigned.
- The DMG's cosmetic Finder-layout step can fail in a headless session; the
  `.app` and a mountable `.dmg` are produced regardless.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). The short version: one phase or one
issue at a time, tests are not optional, and a feature is not done until the
conversion matrix reflects it.

## License

MIT — see [LICENSE](LICENSE).

## Third-party engines

None bundled yet. When FFmpeg, libvips/ImageMagick, libheif and qpdf arrive,
each will be pinned, checksum-verified and credited here with its license.
