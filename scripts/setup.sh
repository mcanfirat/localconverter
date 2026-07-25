#!/usr/bin/env bash
# LocalConvert one-command setup for macOS and Linux.
#
#   ./scripts/setup.sh            # install prerequisites, then build the app
#   ./scripts/setup.sh dev        # install prerequisites, then run it (hot reload)
#   ./scripts/setup.sh --no-media # skip FFmpeg (audio/video stays disabled)
#
# Everything installed here is a build/runtime tool the project needs; no user
# file is touched and nothing but these tools is downloaded. Steps are
# idempotent — re-running is safe.

set -u
cd "$(dirname "$0")/.." || exit 1

MODE="build"
WANT_MEDIA=1
for arg in "$@"; do
  case "$arg" in
    dev) MODE="dev" ;;
    build) MODE="build" ;;
    --no-media) WANT_MEDIA=0 ;;
    *) echo "unknown argument: $arg (use: dev | build | --no-media)"; exit 1 ;;
  esac
done

log()  { printf '\n\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mwarning:\033[0m %s\n' "$*"; }
die()  { printf '\033[1;31merror:\033[0m %s\n' "$*"; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

OS="$(uname -s)"
have curl || die "curl is required to bootstrap. Install curl and re-run."

# --- macOS: Xcode Command Line Tools are required to compile anything --------
if [ "$OS" = "Darwin" ] && ! xcode-select -p >/dev/null 2>&1; then
  log "Installing Apple's Command Line Tools (a system dialog will open)…"
  xcode-select --install >/dev/null 2>&1 || true
  die "Finish the Command Line Tools install in the dialog, then run this again."
fi
# --- macOS: Homebrew is how we fetch Node/FFmpeg if they are missing ---------
if [ "$OS" = "Darwin" ] && ! have brew; then
  warn "Homebrew isn't installed. If Node or FFmpeg turn out to be missing,"
  warn "install Homebrew first, then re-run this script:"
  warn '  /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"'
fi

# Best-effort package install across the common managers.
pkg_install() {
  if have brew;        then brew install "$@"
  elif have apt-get;   then sudo apt-get update -y && sudo apt-get install -y "$@"
  elif have dnf;       then sudo dnf install -y "$@"
  elif have pacman;    then sudo pacman -Sy --noconfirm "$@"
  elif have zypper;    then sudo zypper install -y "$@"
  else return 1
  fi
}

# --- Rust -------------------------------------------------------------------
if ! have cargo; then
  log "Installing the Rust toolchain (rustup)…"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable \
    || die "rustup install failed"
fi
# Make cargo usable in this shell even on a fresh install.
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
have cargo || die "cargo still not on PATH — open a new shell and re-run."

# --- Node -------------------------------------------------------------------
if ! have node; then
  log "Installing Node.js…"
  pkg_install nodejs npm || pkg_install node \
    || die "could not install Node.js automatically — install Node ≥20 from https://nodejs.org and re-run."
fi
NODE_MAJOR="$(node -p 'process.versions.node.split(".")[0]' 2>/dev/null || echo 0)"
[ "$NODE_MAJOR" -ge 20 ] 2>/dev/null || warn "Node $(node -v 2>/dev/null) detected; this project targets Node ≥20."

# --- pnpm (ships with Node via corepack) ------------------------------------
if ! have pnpm; then
  log "Enabling pnpm…"
  if have corepack; then
    corepack enable >/dev/null 2>&1 || true
    corepack prepare pnpm@10 --activate || warn "corepack could not activate pnpm."
  fi
  have pnpm || npm install -g pnpm \
    || die "could not install pnpm — see https://pnpm.io/installation and re-run."
fi

# --- Linux desktop/WebKit build dependencies --------------------------------
if [ "$OS" = "Linux" ]; then
  log "Installing Linux desktop build dependencies…"
  if have apt-get; then
    sudo apt-get install -y \
      libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
      librsvg2-dev patchelf build-essential curl file \
      || warn "some GUI build deps failed to install; a build may fail until they are present."
  elif have dnf; then
    sudo dnf install -y webkit2gtk4.1-devel gtk3-devel libappindicator-gtk3-devel \
      librsvg2-devel patchelf || warn "some GUI build deps failed to install."
  else
    warn "Install your distro's webkit2gtk/gtk3 dev packages if the build fails."
  fi
fi

# --- FFmpeg (optional; enables audio/video) ---------------------------------
if [ "$WANT_MEDIA" -eq 1 ] && ! have ffmpeg; then
  log "Installing FFmpeg (optional — enables audio & video conversion)…"
  pkg_install ffmpeg \
    || warn "FFmpeg not installed. Images, archives, PDF and spreadsheets still work; media needs FFmpeg."
fi

# --- project dependencies ---------------------------------------------------
log "Installing project dependencies (pnpm install)…"
pnpm install || die "pnpm install failed"

# --- build or run -----------------------------------------------------------
if [ "$MODE" = "dev" ]; then
  log "Launching LocalConvert (development mode)…"
  exec pnpm dev
fi

log "Building LocalConvert (this takes a few minutes the first time)…"
pnpm tauri build || die "build failed — see the output above."

log "Done — LocalConvert is built."
case "$OS" in
  Darwin)
    APP="$(/usr/bin/find target/release/bundle/macos -maxdepth 1 -name '*.app' 2>/dev/null | head -1)"
    if [ -n "$APP" ]; then
      # Ad-hoc sign + clear the download flag so macOS opens it without the
      # "unidentified developer / damaged" prompt. (This build is not notarized.)
      codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || true
      xattr -dr com.apple.quarantine "$APP" >/dev/null 2>&1 || true
      echo
      echo "  The app:   $APP"
      echo "  Installer: target/release/bundle/dmg/  (a .dmg you can share)"
      echo
      echo "  To keep it:  drag LocalConvert.app into your Applications folder."
      echo "  If macOS ever says it \"can't be opened\", right-click the app →"
      echo "  Open → Open (only needed once; the build isn't Apple-notarized yet)."
      echo
      echo "  Opening it now so you can try it…"
      open "$APP" 2>/dev/null || warn "Could not auto-open; open it from Finder."
    fi
    ;;
  Linux)
    echo
    echo "  Bundles are under target/release/bundle/  (an AppImage and a .deb)."
    echo "  AppImage: chmod +x the .AppImage and double-click it."
    echo "  .deb:     sudo apt install ./<file>.deb"
    ;;
esac
echo
echo "  Prefer the terminal?  cargo run --release -p localconvert-cli -- --help"
