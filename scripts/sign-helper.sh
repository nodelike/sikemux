#!/usr/bin/env bash
# Sign a published helper as a release is signed:
# hardened runtime and the app's microphone entitlement. APPLE_SIGNING_IDENTITY
# picks the identity; without it the signature is ad hoc.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
IDENTITY="${APPLE_SIGNING_IDENTITY:--}"
APP_IDENTIFIER="$(node "$(dirname "$0")/app-metadata.mjs" identifier)"
# shellcheck source=/dev/null
source "$ROOT/scripts/release-shape.sh"

for helper in "$@"; do
  name="$(basename "$helper" "-$MAC_TARGET")"
  [[ " ${PUBLISHED_HELPERS[*]} " == *" $name "* ]] || { echo "not a helper: $helper" >&2; exit 2; }
  identifier="$APP_IDENTIFIER.${name#sikemux-}"
  [[ -f "$helper" && ! -L "$helper" ]] || { echo "missing helper: $helper" >&2; exit 1; }
  timestamp=()
  [[ "$IDENTITY" != "-" ]] && timestamp=(--timestamp)
  /usr/bin/codesign --force --identifier "$identifier" --options runtime \
    --entitlements "$ROOT/packaging/Entitlements.plist" ${timestamp[@]+"${timestamp[@]}"} \
    --sign "$IDENTITY" "$helper"
  /usr/bin/codesign --verify --strict "$helper"
done
