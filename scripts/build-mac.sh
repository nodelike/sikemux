#!/usr/bin/env bash
# Build and validate the macOS app bundle, ad hoc signed: the GPUI app, its core
# and the notch helper, laid out as every release since the core updater, so an
# installed build updates into it. A release signs it afterwards with
# scripts/sign-mac.sh, on a machine that never runs the build's dependencies.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd -P)"
APP_NAME="$(node scripts/app-metadata.mjs name)"
APP_VERSION="$(node scripts/app-metadata.mjs version)"
APP_IDENTIFIER="$(node scripts/app-metadata.mjs identifier)"
MINIMUM_MACOS="$(node scripts/app-metadata.mjs minimum-macos)"
ICON_ASSET="sikemux"

# shellcheck source=/dev/null
source "$ROOT/scripts/release-shape.sh"
CORE_LINK_TARGET="$(node scripts/app-metadata.mjs core-link)"

unset "${SIGNING_SECRETS[@]}"

# The Release workflow compiles the app and packs it on separate machines, so
# the app compiles while the core does: BUILD_STEP=compile, then BUILD_STEP=package.
STEP="${BUILD_STEP:-all}"
[[ "$STEP" == "all" || "$STEP" == "compile" || "$STEP" == "package" ]] || {
  echo "BUILD_STEP must be compile or package" >&2
  exit 2
}

"$ROOT/scripts/icons.sh"
# A release builds the voice and simulator helpers first and signs them; they
# are published beside the app, never inside it.
if [[ "${FROM_RELEASE:-0}" != "1" && "$STEP" != "package" ]]; then
  node "$ROOT/scripts/build-voice-helper.mjs"
  node "$ROOT/scripts/build-sim-helper.mjs"
fi
GPUI="$ROOT/app"
if [[ "$STEP" != "package" ]]; then
  echo "→ cargo build --release --locked --bin sikemux"
  (cd "$GPUI" && cargo build --release --locked --bin sikemux)
fi
[[ "$STEP" == "compile" ]] && exit 0
node "$ROOT/scripts/build-notch-helper.mjs"
node "$ROOT/scripts/build-core.mjs"

BUNDLE="$ROOT/target/release/bundle"
APP_PATH="$BUNDLE/macos/${APP_NAME}.app"
PLIST="$APP_PATH/Contents/Info.plist"

fail() {
  echo "macOS bundle verification failed: $*" >&2
  exit 1
}

echo "→ Assembling $APP_NAME.app"
rm -rf "$APP_PATH" "$BUNDLE/dmg"
mkdir -p "$APP_PATH/Contents/MacOS" "$APP_PATH/Contents/Resources" "$APP_PATH/Contents/Helpers" "$BUNDLE/dmg"
/usr/bin/ditto "$ROOT/target/release/sikemux" "$APP_PATH/Contents/MacOS/sikemux"
/usr/bin/ditto "$ROOT/packaging/binaries/core/Helpers/Sikemux Core.app" "$APP_PATH/Contents/Helpers/Sikemux Core.app"
/usr/bin/ditto "$ROOT/packaging/binaries/notch/Sikemux Notch.app" "$APP_PATH/Contents/Helpers/Sikemux Notch.app"
ln -s "$CORE_LINK_TARGET" "$APP_PATH/Contents/MacOS/sikemux-host"
/usr/bin/ditto "$ROOT/packaging/icons/build/Assets.car" "$APP_PATH/Contents/Resources/Assets.car"
/usr/bin/ditto "$ROOT/packaging/icons/build/sikemux.icns" "$APP_PATH/Contents/Resources/icon.icns"
/usr/libexec/PlistBuddy \
  -c "Add :CFBundleDevelopmentRegion string English" \
  -c "Add :CFBundleDisplayName string $APP_NAME" \
  -c "Add :CFBundleName string $APP_NAME" \
  -c "Add :CFBundleExecutable string sikemux" \
  -c "Add :CFBundleIconFile string icon.icns" \
  -c "Add :CFBundleIdentifier string $APP_IDENTIFIER" \
  -c "Add :CFBundleInfoDictionaryVersion string 6.0" \
  -c "Add :CFBundlePackageType string APPL" \
  -c "Add :CFBundleShortVersionString string $APP_VERSION" \
  -c "Add :CFBundleVersion string $APP_VERSION" \
  -c "Add :LSMinimumSystemVersion string $MINIMUM_MACOS" \
  -c "Add :NSHighResolutionCapable bool true" \
  -c "Merge $ROOT/packaging/Info.plist" \
  "$PLIST" >/dev/null
/usr/bin/codesign --force --options runtime --entitlements "$ROOT/packaging/Entitlements.plist" --sign - "$APP_PATH"


# The DMG holds what every release's has: the app, a link to Applications, the
# background and the volume's icon, packed with LZMA.
DMG="$BUNDLE/dmg/${APP_NAME}_${APP_VERSION}_aarch64.dmg"
LAYOUT="$(mktemp -d)"
trap 'rm -rf "$LAYOUT"' EXIT
mkdir "$LAYOUT/.background"
/usr/bin/ditto "$APP_PATH" "$LAYOUT/$APP_NAME.app"
ln -s /Applications "$LAYOUT/Applications"
/usr/bin/ditto "$ROOT/packaging/dmg/background.tiff" "$LAYOUT/.background/background.tiff"
/usr/bin/ditto "$ROOT/packaging/icons/build/sikemux.icns" "$LAYOUT/.VolumeIcon.icns"
/usr/bin/hdiutil create -volname "$APP_NAME" -srcfolder "$LAYOUT" -fs HFS+ -format UDRW -ov -quiet "$LAYOUT.rw.dmg" ||
  fail "could not lay out the DMG"
VOLUME="$LAYOUT.volume"
mkdir "$VOLUME"
/usr/bin/hdiutil attach "$LAYOUT.rw.dmg" -readwrite -nobrowse -noautoopen -mountpoint "$VOLUME" -quiet || fail "could not mount the DMG"
/usr/bin/SetFile -a C "$VOLUME" || fail "could not give the DMG its icon"
/usr/bin/hdiutil detach "$VOLUME" -quiet || fail "could not unmount the DMG"
/usr/bin/hdiutil convert "$LAYOUT.rw.dmg" -format ULMO -o "$DMG" -ov -quiet || fail "could not pack $DMG"
rm -rf "$LAYOUT.rw.dmg" "$VOLUME"
DMG_BYTES="$(stat -f%z "$DMG")"
((DMG_BYTES <= DMG_BUDGET_BYTES)) || fail "$DMG is $DMG_BYTES bytes, over the $DMG_BUDGET_BYTES byte budget"

[[ -d "$APP_PATH" ]] || fail "missing app at $APP_PATH"
[[ -f "$PLIST" ]] || fail "missing $PLIST"
[[ -s "$APP_PATH/Contents/Resources/Assets.car" ]] || fail "missing or empty Assets.car"
/usr/bin/cmp -s "$ROOT/packaging/icons/build/Assets.car" "$APP_PATH/Contents/Resources/Assets.car" || fail "bundled Assets.car differs from the generated asset catalog"
/usr/bin/plutil -lint "$PLIST" >/dev/null || fail "invalid Info.plist"

plist_value() {
  /usr/libexec/PlistBuddy -c "Print :$1" "$PLIST" 2>/dev/null || true
}

[[ "$(plist_value CFBundleIconName)" == "$ICON_ASSET" ]] || fail "CFBundleIconName is not $ICON_ASSET"
[[ "$(plist_value CFBundleShortVersionString)" == "$APP_VERSION" ]] || fail "CFBundleShortVersionString does not match $APP_VERSION"
[[ "$(plist_value CFBundleIdentifier)" == "$APP_IDENTIFIER" ]] || fail "CFBundleIdentifier does not match $APP_IDENTIFIER"

EXECUTABLE_NAME="$(plist_value CFBundleExecutable)"
[[ -n "$EXECUTABLE_NAME" ]] || fail "CFBundleExecutable is missing"
EXECUTABLE="$APP_PATH/Contents/MacOS/$EXECUTABLE_NAME"
[[ -x "$EXECUTABLE" ]] || fail "bundle executable is missing or not executable"
ARCHS="$(/usr/bin/lipo -archs "$EXECUTABLE")"
[[ -n "$ARCHS" ]] || fail "could not determine executable architecture"
CORE_APP="$APP_PATH/Contents/Helpers/Sikemux Core.app"
CLI_EXECUTABLE="$CORE_APP/Contents/MacOS/sikemux-host"
[[ -x "$CLI_EXECUTABLE" && ! -L "$CLI_EXECUTABLE" ]] || fail "bundled core is missing or not executable"
[[ "$(readlink "$APP_PATH/Contents/MacOS/sikemux-host")" == "$CORE_LINK_TARGET" ]] ||
  fail "Contents/MacOS/sikemux-host does not link to the bundled core"
[[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$CORE_APP/Contents/Info.plist")" == "$APP_IDENTIFIER.core" ]] ||
  fail "the core's CFBundleIdentifier is not $APP_IDENTIFIER.core"
CORE_SIGNATURE="$(/usr/bin/codesign -dv "$CORE_APP" 2>&1)"
grep -qx "Identifier=$APP_IDENTIFIER.core" <<<"$CORE_SIGNATURE" || fail "the core is not signed as $APP_IDENTIFIER.core"
grep -q 'flags=.*runtime' <<<"$CORE_SIGNATURE" || fail "the core lacks the hardened runtime"
CLI_ARCHS="$(/usr/bin/lipo -archs "$CLI_EXECUTABLE")"
[[ "$CLI_ARCHS" == "$ARCHS" ]] || fail "core architecture ($CLI_ARCHS) differs from app ($ARCHS)"
[[ -s "$CORE_APP/Contents/Resources/sikemux_pi_tools.ts" ]] || fail "bundled Pi browser extension is missing"
# Fails when a binary links a library from the build machine's package manager.
# Such binaries pass codesign verification but fail at launch on user machines
# (or under library-validation Team ID checks).
check_libraries() {
  local libraries
  libraries="$(/usr/bin/otool -L "$1")"
  if grep -Eq '^[[:space:]]+(/opt/homebrew|/usr/local|/opt/local)/' <<<"$libraries"; then
    echo "$libraries" >&2
    fail "$2 links to a package-manager library"
  fi
}
# The published helpers must be built and signed but stay out of the app.
for helper in "${PUBLISHED_HELPERS[@]}"; do
  executable="$ROOT/packaging/binaries/$helper-aarch64-apple-darwin"
  [[ -x "$executable" ]] || fail "$helper is missing or not executable"
  [[ ! -e "$APP_PATH/Contents/MacOS/$helper" ]] || fail "$helper is bundled in the app"
  /usr/bin/codesign --verify --strict "$executable" || fail "$helper signature is invalid"
  grep -q 'flags=.*runtime' <<<"$(/usr/bin/codesign -dv "$executable" 2>&1)" || fail "$helper lacks the hardened runtime"
  [[ "$(/usr/bin/lipo -archs "$executable")" == "$ARCHS" ]] || fail "$helper architecture differs from the app's ($ARCHS)"
  check_libraries "$executable" "$helper"
done
NOTCH_APP="$APP_PATH/Contents/Helpers/Sikemux Notch.app"
NOTCH_EXECUTABLE="$NOTCH_APP/Contents/MacOS/sikemux-notch"
[[ -x "$NOTCH_EXECUTABLE" ]] || fail "notch helper is missing or not executable"
/usr/bin/codesign --verify --strict "$NOTCH_APP" || fail "notch helper signature is invalid"
NOTCH_ARCHS="$(/usr/bin/lipo -archs "$NOTCH_EXECUTABLE")"
[[ "$NOTCH_ARCHS" == "$ARCHS" ]] || fail "notch helper architecture ($NOTCH_ARCHS) differs from app ($ARCHS)"
check_libraries "$EXECUTABLE" "bundle executable"
check_libraries "$CLI_EXECUTABLE" "the core"

# Test kits build the app with `drive` and the core with `fake-plugins`, which answer to
# a socket or to files instead of the person; each leaves this text in what it builds.
for code in "$EXECUTABLE" "$CLI_EXECUTABLE"; do
  if /usr/bin/grep -aqF -e "window driver could not listen" -e "this test kit has no answer for that" "$code"; then
    fail "$code was built with a test kit's feature (drive or fake-plugins)"
  fi
done

# Only starting the bundled core proves it survives the hardened runtime: the
# copy built beside it is signed without it and starts whether or not the
# bundle would. An empty agent id is the earliest thing its tools MCP server
# checks.
TOOLS_START="$(SIKEMUX_TOOLS_AGENT_ID='' "$CLI_EXECUTABLE" --tools-mcp 2>&1 || true)"
if ! grep -Fq "Missing SIKEMUX_TOOLS_AGENT_ID" <<<"$TOOLS_START"; then
  echo "$TOOLS_START" >&2
  fail "bundled tools MCP server does not start"
fi

for helper in "${PUBLISHED_HELPERS[@]}"; do
  "$ROOT/packaging/binaries/$helper-aarch64-apple-darwin" --version | grep -Fq "$helper" || fail "signed $helper does not start"
done

# A release requires a structurally valid ad hoc signature before it signs the app again.
if /usr/bin/codesign --verify --deep --strict --verbose=2 "$APP_PATH" 2>/dev/null; then
  echo "  ✓ code signature is structurally valid"
elif [[ "${FROM_RELEASE:-0}" == "1" ]]; then
  /usr/bin/codesign --verify --deep --strict --verbose=2 "$APP_PATH" >&2 || true
  fail "codesign verification failed"
else
  echo "  ! local app is not fully code signed (release.sh enforces signing)" >&2
fi

echo ""
echo "✓ Verified macOS bundle"
echo "  app: $APP_PATH"
echo "  version: $APP_VERSION"
echo "  architectures: $ARCHS"
echo "  core: $CLI_EXECUTABLE ($CLI_ARCHS)"
