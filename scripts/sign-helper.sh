#!/usr/bin/env bash
# Sign the voice or simulator helper the way the bundler signs what it ships:
# hardened runtime and the app's microphone entitlement. APPLE_SIGNING_IDENTITY
# picks the identity; without it the signature is ad hoc.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
IDENTITY="${APPLE_SIGNING_IDENTITY:--}"

for helper in "$@"; do
  case "$(basename "$helper")" in
    sikemux-voice-*) identifier=com.nodelike.sikemux.voice ;;
    sikemux-sim-*) identifier=com.nodelike.sikemux.sim ;;
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
