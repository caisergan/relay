#!/usr/bin/env bash
#
# Build Relay.app on this Mac and launch it.
#
# A debug build by default: quick to compile, with the webview's inspector enabled.
# `--release` builds what CI ships — the release profile with LTO, so expect several
# minutes — for the current architecture only; CI's universal binary is its own job.
#
# Built here, the bundle carries no quarantine attribute, so it launches straight from
# `target/` without the copy-and-sign dance `install-macos-build.sh` does for a download.
# It is ad-hoc signed by the bundler, as the release workflow does, so its seal verifies.
#
# Usage:
#   scripts/build.sh [--release] [--no-open]
#
#   --release   release profile; the bundle lands in target/release/bundle/macos
#   --no-open   build only; do not quit a running copy or launch the new one

set -euo pipefail

die() { printf 'error: %s\n' "$1" >&2; exit 1; }

usage() { sed -n '3,17s/^# \{0,1\}//p' "$0"; }

profile=debug
launch=true
for arg in "$@"; do
  case "$arg" in
    --release) profile=release ;;
    --no-open) launch=false ;;
    -h | --help) usage; exit 0 ;;
    *) usage >&2; die "unknown option: $arg" ;;
  esac
done

[[ "$(uname -s)" == "Darwin" ]] || die "macOS only; this is $(uname -s)."
command -v pnpm >/dev/null || die "pnpm is not installed; see package.json for the version."

cd "$(dirname "$0")/.."

# A fresh clone, or a lockfile that moved on: the build would fail on the first import.
if [[ ! -d node_modules ]]; then
  pnpm install --frozen-lockfile
fi

flags=(--bundles app)
[[ "$profile" == debug ]] && flags+=(--debug)

printf 'Building Relay (%s)...\n' "$profile"
started=$SECONDS
APPLE_SIGNING_IDENTITY="${APPLE_SIGNING_IDENTITY:--}" pnpm tauri build "${flags[@]}"

app="$PWD/target/$profile/bundle/macos/Relay.app"
[[ -d "$app" ]] || die "the build finished but there is no bundle at $app"
codesign --verify --strict "$app" || die "the bundle's signature does not verify: $app"
printf '\nBuilt %s in %ss\n' "$app" "$(( SECONDS - started ))"

$launch || exit 0

# `open` on a bundle that is already running brings the old process forward instead of
# starting the new build, so a copy launched from this same path is quit first.
if pkill -f "$app/Contents/MacOS/" 2>/dev/null; then
  printf 'Quit the copy that was running.\n'
  sleep 1
fi
open "$app"
printf 'Launched.\n'
