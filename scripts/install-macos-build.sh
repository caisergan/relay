#!/usr/bin/env bash
#
# Install a CI macOS build so it will actually launch.
#
# A Relay.app unzipped straight from a CI artifact usually will not start. The
# icon bounces until launchd gives up: no dialog, no crash report, nothing in
# the log. The process sits at _dyld_start having never run an instruction,
# ~32 KB resident, while a working launch settles above 100 MB.
#
# The cause is that the downloaded bundle arrives quarantined, and Relay is only
# ad-hoc signed (real signing and notarisation are phase 5), so the security
# assessment fails. The verdict is then cached against that bundle's identity —
# LaunchServices keys the registration by inode, visible in the log as
# application.app.relay.desktop.<inode>.<n> — and clearing com.apple.quarantine
# or re-signing the bundle in place does NOT clear it. The stalled bundle stays
# stalled for as long as it keeps that inode.
#
# What does work is giving the app a new file identity. This script copies the
# bundle to a fresh path before signing it, which is why it succeeds where
# `xattr -dr com.apple.quarantine Relay.app` on the original does not. The
# directory itself is irrelevant: a fresh copy runs even from ~/Downloads. It is
# the reused inode, not the location, that is blocked.
#
# None of this is a bug in the build — the same bytes run fine once copied.
#
# Usage:
#   scripts/install-macos-build.sh [path-to-Relay-app.zip-or-Relay.app]
#
# With no argument it picks the newest Relay-app.zip or Relay.app in ~/Downloads.

set -euo pipefail

DEST_DIR="${RELAY_DEST_DIR:-$HOME/Applications}"
APP_NAME="Relay.app"

die() { printf 'error: %s\n' "$1" >&2; exit 1; }

[[ "$(uname -s)" == "Darwin" ]] || die "macOS only; this is $(uname -s)."

src="${1:-}"
if [[ -z "$src" ]]; then
  src="$(ls -td "$HOME"/Downloads/Relay-app.zip "$HOME"/Downloads/"$APP_NAME" 2>/dev/null | head -1 || true)"
  [[ -n "$src" ]] || die "no Relay-app.zip or $APP_NAME in ~/Downloads; pass one as an argument."
  printf 'Using %s\n' "$src"
fi
[[ -e "$src" ]] || die "no such file: $src"

staging="$(mktemp -d)"
trap 'rm -rf "$staging"' EXIT

case "$src" in
  *.zip)
    # ditto, not unzip: unzip drops the executable bit and the bundle structure.
    ditto -x -k "$src" "$staging" || die "could not expand $src"
    app="$(find "$staging" -maxdepth 2 -name "$APP_NAME" -type d | head -1)"
    [[ -n "$app" ]] || die "no $APP_NAME inside $src"
    ;;
  *.app)
    app="$src"
    ;;
  *)
    die "expected a .zip or a .app, got $src"
    ;;
esac

mkdir -p "$DEST_DIR"
target="$DEST_DIR/$APP_NAME"

if [[ -e "$target" ]]; then
  # Quit a running copy first; replacing the bundle under a live process leaves
  # it in a state where the next launch fails for a different reason.
  pkill -f "$target/Contents/MacOS/" 2>/dev/null || true
  # rm, not overwrite: the point of the copy is a new inode. Writing over the
  # existing bundle would inherit the old identity and any verdict cached
  # against it, which is the failure this script exists to avoid.
  rm -rf "$target"
fi

ditto "$app" "$target"

# Order matters: strip the quarantine and provenance attributes first, then sign.
# Signing a bundle that still carries them leaves the seal covering the old state.
xattr -cr "$target" 2>/dev/null || true
codesign --force --deep --sign - "$target"
codesign --verify --strict "$target" || die "signature did not verify after install"

printf '\nInstalled to %s\n' "$target"
printf 'Launching...\n'
open "$target"

# A working launch reaches a webview and settles well above 100 MB resident; a
# blocked one sits at ~32 KB, stalled at _dyld_start. Report which happened
# rather than claiming success on the strength of `open` returning 0.
sleep 5
rss="$(ps -o rss= -p "$(pgrep -f "$target/Contents/MacOS/" | head -1)" 2>/dev/null | tr -d ' ' || true)"
if [[ -z "$rss" ]]; then
  die "the app exited immediately; run it from a terminal to see stderr."
elif (( rss < 51200 )); then
  die "the app is stalled at launch (${rss} KB resident). It is being blocked, not crashing."
else
  printf 'Running (%s MB resident).\n' "$(( rss / 1024 ))"
fi
