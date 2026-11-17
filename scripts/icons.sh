#!/usr/bin/env bash
#
# Compile sikemux.icon → Liquid Glass Assets.car + legacy .icns under
# packaging/icons/build, in one pass, for build-mac.sh to put in the bundle.
#
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd -P)"
ICON_SOURCE="$ROOT/packaging/icons/sikemux.icon"
ICON_BUILD="$ROOT/packaging/icons/build"

# actool takes a couple of seconds, so only a changed .icon source reruns it.
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
  --minimum-deployment-target "$(node scripts/app-metadata.mjs minimum-macos)" \
  --platform macosx >/dev/null

if [[ ! -f "$ICON_BUILD/Assets.car" ]]; then
  echo "actool did not produce Assets.car" >&2
  exit 1
fi

touch "$STAMP"
echo "  ✓ Assets.car + sikemux.icns built from sikemux.icon"
