#!/usr/bin/env bash
# Publish a release that scripts/release.sh built and verified.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd -P)"
VERSION="${1:-}"
NOTES="${2:-}"
CHANNEL=stable
[[ "${3:-}" == "--nightly" ]] && CHANNEL=nightly

fail() {
  echo "Release publish failed: $*" >&2
  exit 1
}

[[ -n "$VERSION" && -n "$NOTES" ]] || { echo "Usage: $0 <version> <notes> [--nightly]" >&2; exit 2; }
[[ "${GITHUB_ACTIONS:-}" == "true" ]] || fail "releases publish from the Release workflow"

BUNDLE="$ROOT/src-tauri/target/release/bundle"
APP_NAME="$(node -p "require('./src-tauri/tauri.conf.json').productName")"
TAR="$BUNDLE/macos/${APP_NAME}.app.tar.gz"
SIG="$TAR.sig"
if [[ "$CHANNEL" == "stable" ]]; then
  MANIFEST="$ROOT/latest.json"
else
  MANIFEST="$BUNDLE/latest.json"
fi
shopt -s nullglob
DMGS=("$BUNDLE"/dmg/*.dmg)
VOICES=("$ROOT"/src-tauri/binaries/sikemux-voice-*)
SIMS=("$ROOT"/src-tauri/binaries/sikemux-sim-*)
shopt -u nullglob
[[ ${#DMGS[@]} -eq 1 ]] || fail "expected exactly one DMG, found ${#DMGS[@]}"
[[ ${#VOICES[@]} -eq 1 ]] || fail "expected exactly one voice helper, found ${#VOICES[@]}"
[[ ${#SIMS[@]} -eq 1 ]] || fail "expected exactly one simulator helper, found ${#SIMS[@]}"
DMG="${DMGS[0]}"
VOICE="${VOICES[0]}"
SIM="${SIMS[0]}"
for file in "$DMG" "$TAR" "$SIG" "$VOICE" "$SIM" "$MANIFEST"; do
  [[ -s "$file" ]] || fail "$file is missing or empty"
done
[[ "$(node -p "require('$MANIFEST').version")" == "$VERSION" ]] || fail "$MANIFEST is not for v$VERSION"

STABLE_GH_CMD=(gh release create "v$VERSION" --verify-tag --title "v$VERSION" --notes "$NOTES" "$DMG" "$TAR" "$SIG" "$VOICE" "$SIM" "$MANIFEST")
NIGHTLY_GH_CMD=(gh release create "v$VERSION" --verify-tag --title "v$VERSION" --notes "$NOTES" --prerelease "$DMG" "$TAR" "$SIG" "$VOICE" "$SIM")
POINTER_NOTES="Update feed for the nightly channel.

The installable build for this feed is [v$VERSION](https://github.com/nodelike/sikemux/releases/tag/v$VERSION).

This release carries only \`latest.json\`. Its \`nightly\` tag is a fixed URL anchor that shipped clients resolve against, not a source revision — do not attach builds here."
if [[ "$CHANNEL" == "stable" ]]; then
  echo "→ Publishing stable v$VERSION"
  "${STABLE_GH_CMD[@]}"
  echo "✓ Released stable v$VERSION"
else
  echo "→ Publishing nightly v$VERSION"
  "${NIGHTLY_GH_CMD[@]}"
  if gh release view nightly >/dev/null 2>&1; then
    gh release upload nightly "$MANIFEST" --clobber
    gh release edit nightly --title "Nightly feed (v$VERSION)" --notes "$POINTER_NOTES" --prerelease
  else
    gh release create nightly --target "$(git rev-parse HEAD)" --title "Nightly feed (v$VERSION)" --notes "$POINTER_NOTES" --prerelease "$MANIFEST"
  fi
  echo "✓ Released nightly v$VERSION; nightly feed now points at v$VERSION"
fi
