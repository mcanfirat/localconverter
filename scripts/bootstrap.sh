#!/usr/bin/env bash
# LocalConvert one-line bootstrap for macOS and Linux.
#
#   curl -fsSL <RAW_URL>/scripts/bootstrap.sh | bash
#
# Clones the project, installs the build tools, compiles the app and opens it.
# Everything after this runs entirely on your machine — no file you convert ever
# leaves it.
#
# Override where it clones from / to:
#   LOCALCONVERT_REPO=https://github.com/you/localconvert.git
#   LOCALCONVERT_DIR=~/apps/localconvert

set -eu

# Replace when you publish.
DEFAULT_REPO="https://github.com/mcanfirat/localconverter.git"
REPO="${LOCALCONVERT_REPO:-$DEFAULT_REPO}"
TARGET="${LOCALCONVERT_DIR:-$HOME/localconvert}"

log() { printf '\n\033[1;34m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

command -v git >/dev/null 2>&1 \
  || die "git is required. macOS: xcode-select --install   Linux: install the 'git' package."

if [ -d "$TARGET/.git" ]; then
  log "Updating the existing copy at $TARGET…"
  # Never discard local edits — if the pull conflicts, build what is there.
  git -C "$TARGET" pull --ff-only >/dev/null 2>&1 \
    || log "Local changes present; building the current checkout without pulling."
elif [ -e "$TARGET" ]; then
  die "$TARGET already exists and is not a git checkout.
       Move it, or set LOCALCONVERT_DIR to somewhere else."
else
  log "Cloning LocalConvert into $TARGET…"
  mkdir -p "$(dirname "$TARGET")"
  git clone --depth 1 "$REPO" "$TARGET" \
    || die "could not clone $REPO — check the address and your connection."
fi

[ -f "$TARGET/scripts/setup.sh" ] \
  || die "$TARGET does not look like LocalConvert (no scripts/setup.sh inside)."

log "Building — this takes a few minutes the first time."
exec bash "$TARGET/scripts/setup.sh" "$@"
