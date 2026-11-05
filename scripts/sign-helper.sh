#!/usr/bin/env bash
# Sign the voice or simulator helper as a release is signed:
# hardened runtime and the app's microphone entitlement. APPLE_SIGNING_IDENTITY
# picks the identity; without it the signature is ad hoc.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
IDENTITY="${APPLE_SIGNING_IDENTITY:--}"
APP_IDENTIFIER="$(node "$(dirname "$0")/app-metadata.mjs" identifier)"

for helper in "$@"; do
  case "$(basename "$helper")" in
    sikemux-voice-*) identifier="$APP_IDENTIFIER.voice" ;;
    sikemux-sim-*) identifier="$APP_IDENTIFIER.sim" ;;
    *) echo "not a helper: $helper" >&2; exit 2 ;;
  esac
  [[ -f "$helper" && ! -L "$helper" ]] || { echo "missing helper: $helper" >&2; exit 1; }
  timestamp=()
  [[ "$IDENTITY" != "-" ]] && timestamp=(--timestamp)
  /usr/bin/codesign --force --identifier "$identifier" --options runtime \
    --entitlements "$ROOT/packaging/Entitlements.plist" ${timestamp[@]+"${timestamp[@]}"} \
    --sign "$IDENTITY" "$helper"
  /usr/bin/codesign --verify --strict "$helper"
done
