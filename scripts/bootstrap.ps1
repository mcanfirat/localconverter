# LocalConvert one-line bootstrap for Windows (PowerShell).
#
#   irm <RAW_URL>/scripts/bootstrap.ps1 | iex
#
# Clones the project, installs the build tools, compiles the app and opens it.
# Everything after this runs entirely on your machine — no file you convert ever
# leaves it.
#
# Override where it clones from / to:
#   $env:LOCALCONVERT_REPO = "https://github.com/you/localconvert.git"
#   $env:LOCALCONVERT_DIR  = "C:\apps\localconvert"
#
# NOTE: mirrors bootstrap.sh but has not yet been run on real Windows hardware.

$ErrorActionPreference = "Stop"

# Replace when you publish.
$DefaultRepo = "https://github.com/mcanfirat/localconverter.git"
$Repo   = if ($env:LOCALCONVERT_REPO) { $env:LOCALCONVERT_REPO } else { $DefaultRepo }
$Target = if ($env:LOCALCONVERT_DIR)  { $env:LOCALCONVERT_DIR }  else { Join-Path $HOME "localconvert" }

function Log($m) { Write-Host "`n==> $m" -ForegroundColor Blue }
function Die($m) { Write-Host "error: $m" -ForegroundColor Red; exit 1 }

if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
  Die "git is required. Install it with:  winget install --id Git.Git -e"
}

if (Test-Path (Join-Path $Target ".git")) {
  Log "Updating the existing copy at $Target…"
  # Never discard local edits — if the pull conflicts, build what is there.
  git -C $Target pull --ff-only 2>$null | Out-Null
  if ($LASTEXITCODE -ne 0) { Log "Local changes present; building the current checkout without pulling." }
}
elseif (Test-Path $Target) {
  Die "$Target already exists and is not a git checkout. Move it, or set `$env:LOCALCONVERT_DIR."
}
else {
  Log "Cloning LocalConvert into $Target…"
  $parent = Split-Path -Parent $Target
  if ($parent -and -not (Test-Path $parent)) { New-Item -ItemType Directory -Path $parent -Force | Out-Null }
  git clone --depth 1 $Repo $Target
  if ($LASTEXITCODE -ne 0) { Die "could not clone $Repo — check the address and your connection." }
}

$setup = Join-Path $Target "scripts\setup.ps1"
if (-not (Test-Path $setup)) { Die "$Target does not look like LocalConvert (no scripts\setup.ps1 inside)." }

Log "Building — this takes a few minutes the first time."
& powershell -ExecutionPolicy Bypass -File $setup @args
exit $LASTEXITCODE
