#!/usr/bin/env bash
#
# Compile sikemux.icon → Liquid Glass Assets.car + legacy .icns, in one
# pass. Called from macOS `make dev` (via tauri.macos.conf.json) and
# `make build` (via build-mac.sh), so the same
# `src-tauri/icons/sikemux.icon` is the single source of truth for
# every icon surface:
#
#   * Dev binary (target/debug/sikemux):
#       embeds icon.icns via tauri-build's build.rs. We overwrite that
#       .icns with the actool-generated one so the dev dock icon matches
#       release geometry/padding (flat fallback, no Liquid Glass effect).
#
#   * Release bundle (target/release/bundle/macos/sikemux.app):
#       tauri.macos.conf.json copies Assets.car into Resources/ and merges
#       CFBundleIconName=sikemux from Info.plist before code signing.
#
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd -P)"
ICON_SOURCE="$ROOT/src-tauri/icons/sikemux.icon"
ICON_BUILD="$ROOT/src-tauri/icons/build"

# Skip the rebuild when the .icon source hasn't changed since the last
# compile — actool takes a couple of seconds and we don't want to add
# that to every `tauri dev` restart loop.
STAMP="$ICON_BUILD/.stamp"
if [[ -f "$STAMP" ]]; then
  NEWEST_SRC="$(find "$ICON_SOURCE" -type f -newer "$STAMP" -print -quit)"
  if [[ -z "$NEWEST_SRC" && ! "$0" -nt "$STAMP" ]]; then
    exit 0
  fi
fi

echo "→ Compiling $ICON_SOURCE"
rm -rf "$ICON_BUILD"
mkdir -p "$ICON_BUILD"

# Space optimisation stores the flat icons for macOS before 26 with a denser
# codec. They are most of Assets.car, and it takes about 250 KB off the DMG.
actool "$ICON_SOURCE" \
  --compile "$ICON_BUILD" \
  --optimization space \
  --output-format human-readable-text \
  --notices --warnings --errors \
  --output-partial-info-plist "$ICON_BUILD/PartialInfo.plist" \
  --app-icon sikemux \
  --include-all-app-icons \
  --enable-on-demand-resources NO \
  --development-region en \
  --target-device mac \
  --minimum-deployment-target 11.0 \
  --platform macosx >/dev/null

if [[ ! -f "$ICON_BUILD/Assets.car" ]]; then
  echo "actool did not produce Assets.car" >&2
  exit 1
fi

# Sync the legacy .icns the dev binary embeds via tauri-build.
if [[ -f "$ICON_BUILD/sikemux.icns" ]]; then
  cp "$ICON_BUILD/sikemux.icns" "$ROOT/src-tauri/icons/icon.icns"
fi

touch "$STAMP"
echo "  ✓ Assets.car + icon.icns synced from sikemux.icon"
