# LocalConvert one-command setup for Windows (PowerShell).
#
#   powershell -ExecutionPolicy Bypass -File scripts\setup.ps1          # build
#   powershell -ExecutionPolicy Bypass -File scripts\setup.ps1 -Dev     # run
#   powershell -ExecutionPolicy Bypass -File scripts\setup.ps1 -NoMedia # skip FFmpeg
#
# Installs the build/runtime tools the project needs via winget, then builds.
# Idempotent — re-running is safe. NOTE: written to mirror setup.sh but has not
# yet been run on real Windows hardware; report issues if a step fails.

param(
  [switch]$Dev,
  [switch]$NoMedia
)

$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

function Log($m)  { Write-Host "`n==> $m" -ForegroundColor Blue }
function Warn($m) { Write-Host "warning: $m" -ForegroundColor Yellow }
function Have($c) { $null -ne (Get-Command $c -ErrorAction SilentlyContinue) }

if (-not (Have "winget")) {
  Warn "winget not found. Install 'App Installer' from the Microsoft Store, or install Rust/Node/FFmpeg manually, then re-run."
}

# --- MSVC C++ build tools ---
# Rust's default Windows toolchain links with MSVC, so without these the very
# first `cargo build` fails with "linker `link.exe` not found". This is the most
# common Windows blocker, so it is installed before Rust rather than after.
if (-not (Have "cl") -and -not (Test-Path "${env:ProgramFiles(x86)}\Microsoft Visual Studio\2022")) {
  Log "Installing Microsoft C++ Build Tools (large download, several minutes)…"
  if (Have "winget") {
    winget install --id Microsoft.VisualStudio.2022.BuildTools -e `
      --accept-source-agreements --accept-package-agreements `
      --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
  } else {
    Warn "Install 'Build Tools for Visual Studio' with the 'Desktop development with C++' workload from https://visualstudio.microsoft.com/downloads/ and re-run."
  }
}

# --- WebView2 (the engine the window renders with) ---
# Bundled with Windows 11 and with any recent Edge; Windows 10 installs that
# never updated Edge can be missing it, and the built app then shows a blank
# window rather than an error.
$webview = @(
  "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}",
  "HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"
)
if (-not ($webview | Where-Object { Test-Path $_ })) {
  Log "Installing the WebView2 runtime…"
  if (Have "winget") {
    winget install --id Microsoft.EdgeWebView2Runtime -e --accept-source-agreements --accept-package-agreements
  } else {
    Warn "Install the WebView2 runtime from https://developer.microsoft.com/microsoft-edge/webview2/ and re-run."
  }
}

# --- Rust ---
if (-not (Have "cargo")) {
  Log "Installing the Rust toolchain…"
  if (Have "winget") { winget install --id Rustlang.Rustup -e --accept-source-agreements --accept-package-agreements }
  else { Warn "Install Rust from https://rustup.rs and re-run." }
  $env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
}

# --- Node ---
if (-not (Have "node")) {
  Log "Installing Node.js LTS…"
  if (Have "winget") { winget install --id OpenJS.NodeJS.LTS -e --accept-source-agreements --accept-package-agreements }
  else { Warn "Install Node.js >=20 from https://nodejs.org and re-run." }
}

# --- pnpm (via corepack) ---
if (-not (Have "pnpm")) {
  Log "Enabling pnpm…"
  if (Have "corepack") { corepack enable; corepack prepare pnpm@10 --activate }
  elseif (Have "npm")  { npm install -g pnpm }
  else { Warn "Install pnpm from https://pnpm.io/installation and re-run." }
}

# --- FFmpeg (optional) ---
if (-not $NoMedia -and -not (Have "ffmpeg")) {
  Log "Installing FFmpeg (optional — enables audio & video)…"
  if (Have "winget") { winget install --id Gyan.FFmpeg -e --accept-source-agreements --accept-package-agreements }
  else { Warn "FFmpeg not installed; media conversion will be disabled until it is on PATH." }
}

# --- project dependencies ---
Log "Installing project dependencies (pnpm install)…"
pnpm install

# --- build or run ---
if ($Dev) {
  Log "Launching LocalConvert (development mode)…"
  pnpm dev
} else {
  Log "Building LocalConvert (this takes a few minutes the first time)…"
  pnpm tauri build
  Log "Done. Installers are under target\release\bundle\  (MSI and NSIS .exe)."
  Write-Host "  CLI:  cargo run --release -p localconvert-cli -- --help"
}
