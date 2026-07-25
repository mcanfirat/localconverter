# Packaging

## Build

```bash
pnpm install
pnpm tauri build            # release bundle for the host platform
pnpm tauri build --debug    # faster, unsigned, for smoke testing
```

The `localconvert` CLI binary lands in `target/release/localconvert`.
Desktop artifacts land in `target/release/bundle/` — the Cargo target directory is at the
workspace root, not under `src-tauri/`.

## Toolchain

| | |
|---|---|
| Rust | stable, ≥ 1.77 (`rust-version` in the workspace manifest) |
| Node | ≥ 20 |
| pnpm | 10.x |

Platform prerequisites: Xcode command line tools on macOS; WebView2 (present on
Windows 11, bootstrapped by the installer on Windows 10) plus the MSVC toolchain
on Windows; `libwebkit2gtk-4.1-dev`, `libgtk-3-dev`, `libayatana-appindicator3-dev`,
`librsvg2-dev`, `patchelf` on Debian/Ubuntu.

## Targets

| Platform | Bundles | Verified |
|---|---|---|
| macOS ARM (13+) | `.app`, `.dmg` | ✅ release bundle built |
| macOS Intel | `.app`, `.dmg` | ⏳ CI only |
| Windows x64 | `.msi`, NSIS `.exe` | ⏳ CI only |
| Linux x64 | `.deb`, AppImage | ⏳ CI only |

`.rpm` is not configured yet.

## Icons

Generated from source rather than committed as an opaque blob:

```bash
node scripts/generate-icon.mjs /tmp/icon-source.png
cd apps/desktop && pnpm tauri icon /tmp/icon-source.png
```

The script draws a 1024×1024 RGBA buffer and deflates it into a PNG with
`node:zlib` — no image dependency. `tauri icon` then produces the `.icns`,
`.ico` and PNG set. iOS/Android output is deleted; this is a desktop app.

## Signing and notarization

Not configured — no credentials exist for this repository. The release workflow
picks them up from secrets when they are added.

**macOS.** Set `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`,
`APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID`. Tauri
signs and submits for notarization during `tauri build`. Hardened runtime is on
by default for signed builds. Without these, the `.dmg` is unsigned and users
see Gatekeeper's "unidentified developer" dialog.

**Windows.** Set `WINDOWS_CERTIFICATE` and `WINDOWS_CERTIFICATE_PASSWORD`.
Unsigned installers trigger SmartScreen until reputation accrues.

**Linux.** No signing. The AppImage and `.deb` are published with SHA-256 sums.

## Third-party engines

None are bundled yet — Phase 0 has no external binaries. When FFmpeg,
libvips/ImageMagick, libheif and qpdf arrive, each gets an entry in
`vendor/manifests/<platform>.json` recording name, version, filename, SHA-256,
license and upstream URL, and the download script verifies the checksum before
unpacking, packaging verifies it again, and the process runner verifies it once
more before execution.

**Licensing to resolve before shipping media support:** FFmpeg builds vary
between LGPL and GPL depending on their configure flags. A GPL build changes the
license obligations for the whole distributed bundle. Pick and document the
build variant before Phase 5 ships, not after.

## Reproducibility

`Cargo.lock` and `pnpm-lock.yaml` are committed. The release profile sets
`lto = true`, `codegen-units = 1`, `strip = true`. Byte-identical rebuilds are
not claimed — timestamps and signing prevent it — but dependency versions are
pinned and an SBOM is generated per release.

## Release checklist

1. `pnpm check` green on all four CI platforms.
2. `docs/CONVERSION_MATRIX.md` matches what CI actually verified.
3. Version bumped in the workspace `Cargo.toml`, both `package.json` files and
   `tauri.conf.json`.
4. Build on each platform, run the pipeline self-test from the installed app.
5. Generate SBOM and SHA-256 sums.
6. Publish with release notes and known limitations.

Blockers are listed in §30 of the specification. Any one of them stops a release.
