#!/usr/bin/env bash
# Makes the Sentry release a version's crashes are filed under, and sends it the debug files that
# name their frames: the Mac dSYMs and the Linux debug files the Release workflow kept.
#   SENTRY_AUTH_TOKEN=… scripts/sentry-release.sh <version> <folder of symbols>
set -euo pipefail
VERSION="${1:?usage: $0 <version> <folder of symbols>}"
SYMBOLS="${2:?usage: $0 <version> <folder of symbols>}"
[[ -n "${SENTRY_AUTH_TOKEN:-}" ]] || { echo "SENTRY_AUTH_TOKEN is not set" >&2; exit 1; }
export SENTRY_ORG=nodelike

sentry() { npx --yes @sentry/cli@3.8.2 "$@"; }

shopt -s nullglob
NOTCH=("$SYMBOLS"/*.notch.dSYM.zip)
DESKTOP=()
for file in "$SYMBOLS"/*.dSYM.zip "$SYMBOLS"/*.debug; do
  [[ "$file" == *.notch.dSYM.zip ]] || DESKTOP+=("$file")
done
[[ ${#NOTCH[@]} -eq 1 ]] || { echo "expected the notch helper's dSYM, found ${#NOTCH[@]}" >&2; exit 1; }
[[ ${#DESKTOP[@]} -gt 0 ]] || { echo "no symbols for the app, the core or the CLI in $SYMBOLS" >&2; exit 1; }

RELEASE="sikemux@$VERSION"
sentry releases new "$RELEASE" --project sikemux-desktop --project sikemux-notch
sentry debug-files upload --project sikemux-desktop --wait "${DESKTOP[@]}"
sentry debug-files upload --project sikemux-notch --wait "${NOTCH[@]}"
sentry releases finalize "$RELEASE"
