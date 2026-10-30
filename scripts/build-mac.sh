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
TARGET=""

args=("$@")
for ((i = 0; i < ${#args[@]}; i++)); do
  case "${args[$i]}" in
    --target)
      if ((i + 1 >= ${#args[@]})); then
        echo "--target requires a value" >&2
        exit 2
      fi
      TARGET="${args[$((i + 1))]}"
      ;;
    --target=*) TARGET="${args[$i]#--target=}" ;;
  esac
done

# No signing material reaches the build.
unset TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD
unset APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID APPLE_API_KEY APPLE_API_ISSUER APPLE_API_KEY_PATH

# The Release workflow compiles the app and packs it on separate machines, so
# the app compiles while the core does: BUILD_STEP=compile, then BUILD_STEP=package.
STEP="${BUILD_STEP:-all}"
[[ "$STEP" == "all" || "$STEP" == "compile" || "$STEP" == "package" ]] || {
  echo "BUILD_STEP must be compile or package" >&2
  exit 2
}

"$ROOT/scripts/icons.sh"
TARGET_ARGS=()
[[ -n "$TARGET" ]] && TARGET_ARGS=(--target "$TARGET")
# A release builds the voice and simulator helpers first and signs them; they
# are published beside the app, never inside it.
if [[ "${PREBUILT_HELPERS:-0}" != "1" && "$STEP" != "package" ]]; then
  node "$ROOT/scripts/build-voice-helper.mjs" ${TARGET_ARGS[@]+"${TARGET_ARGS[@]}"}
  node "$ROOT/scripts/build-sim-helper.mjs" ${TARGET_ARGS[@]+"${TARGET_ARGS[@]}"}
fi
GPUI="$ROOT/src-tauri/crates/sikemux-gpui"
TARGET_ROOT="$ROOT/src-tauri/target"
[[ -n "$TARGET" ]] && TARGET_ROOT="$TARGET_ROOT/$TARGET"
if [[ "$STEP" != "package" ]]; then
  echo "→ cargo build --release --locked --bin sikemux ${TARGET_ARGS[*]-}"
  (cd "$GPUI" && cargo build --release --locked --bin sikemux ${TARGET_ARGS[@]+"${TARGET_ARGS[@]}"})
fi
[[ "$STEP" == "compile" ]] && exit 0
node "$ROOT/scripts/build-notch-helper.mjs" ${TARGET_ARGS[@]+"${TARGET_ARGS[@]}"}
node "$ROOT/scripts/build-cli-sidecar.mjs" ${TARGET_ARGS[@]+"${TARGET_ARGS[@]}"}

BUNDLE="$TARGET_ROOT/release/bundle"
APP_PATH="$BUNDLE/macos/${APP_NAME}.app"
PLIST="$APP_PATH/Contents/Info.plist"

fail() {
  echo "macOS bundle verification failed: $*" >&2
  exit 1
}

echo "→ Assembling $APP_NAME.app"
rm -rf "$APP_PATH" "$BUNDLE/dmg"
mkdir -p "$APP_PATH/Contents/MacOS" "$APP_PATH/Contents/Resources" "$APP_PATH/Contents/Helpers" "$BUNDLE/dmg"
/usr/bin/ditto "$TARGET_ROOT/release/sikemux" "$APP_PATH/Contents/MacOS/sikemux"
/usr/bin/ditto "$ROOT/src-tauri/binaries/core/Helpers/Sikemux Core.app" "$APP_PATH/Contents/Helpers/Sikemux Core.app"
/usr/bin/ditto "$ROOT/src-tauri/binaries/notch/Sikemux Notch.app" "$APP_PATH/Contents/Helpers/Sikemux Notch.app"
ln -s "../Helpers/Sikemux Core.app/Contents/MacOS/sikemux-host" "$APP_PATH/Contents/MacOS/sikemux-host"
/usr/bin/ditto "$ROOT/src-tauri/icons/build/Assets.car" "$APP_PATH/Contents/Resources/Assets.car"
/usr/bin/ditto "$ROOT/src-tauri/icons/icon.icns" "$APP_PATH/Contents/Resources/icon.icns"
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
  -c "Merge $ROOT/src-tauri/Info.plist" \
  "$PLIST" >/dev/null
/usr/bin/codesign --force --options runtime --entitlements "$ROOT/src-tauri/Entitlements.plist" --sign - "$APP_PATH"

# The README and sikemux.com promise a small download, so a release that grows
# past this fails here instead of shipping. scripts/sign-mac.sh checks it again.
DMG_BUDGET_BYTES=25000000

# The DMG holds what every release's has: the app, a link to Applications, the
# background and the volume's icon, packed with LZMA.
ARCH="$(/usr/bin/lipo -archs "$APP_PATH/Contents/MacOS/sikemux" | tr ' ' '\n' | sed -e 's/^arm64$/aarch64/' -e 's/^x86_64$/x64/' | head -1)"
DMG="$BUNDLE/dmg/${APP_NAME}_${APP_VERSION}_${ARCH}.dmg"
LAYOUT="$(mktemp -d)"
trap 'rm -rf "$LAYOUT"' EXIT
mkdir "$LAYOUT/.background"
/usr/bin/ditto "$APP_PATH" "$LAYOUT/$APP_NAME.app"
ln -s /Applications "$LAYOUT/Applications"
/usr/bin/ditto "$ROOT/src-tauri/dmg/background.tiff" "$LAYOUT/.background/background.tiff"
/usr/bin/ditto "$ROOT/src-tauri/icons/icon.icns" "$LAYOUT/.VolumeIcon.icns"
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
/usr/bin/cmp -s "$ROOT/src-tauri/icons/build/Assets.car" "$APP_PATH/Contents/Resources/Assets.car" || fail "bundled Assets.car differs from the generated asset catalog"
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
[[ "$(readlink "$APP_PATH/Contents/MacOS/sikemux-host")" == "../Helpers/Sikemux Core.app/Contents/MacOS/sikemux-host" ]] ||
  fail "Contents/MacOS/sikemux-host does not link to the bundled core"
[[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$CORE_APP/Contents/Info.plist")" == "$APP_IDENTIFIER.core" ]] ||
  fail "the core's CFBundleIdentifier is not $APP_IDENTIFIER.core"
CORE_SIGNATURE="$(/usr/bin/codesign -dv "$CORE_APP" 2>&1)"
grep -qx "Identifier=$APP_IDENTIFIER.core" <<<"$CORE_SIGNATURE" || fail "the core is not signed as $APP_IDENTIFIER.core"
grep -q 'flags=.*runtime' <<<"$CORE_SIGNATURE" || fail "the core lacks the hardened runtime"
CLI_ARCHS="$(/usr/bin/lipo -archs "$CLI_EXECUTABLE")"
[[ "$CLI_ARCHS" == "$ARCHS" ]] || fail "core architecture ($CLI_ARCHS) differs from app ($ARCHS)"
[[ -s "$CORE_APP/Contents/Resources/sikemux_pi_tools.ts" ]] || fail "bundled Pi browser extension is missing"
# The voice helper is published beside the release and downloaded with the
# speech model, so it must be built and signed but stay out of the app.
VOICE_TARGET="${TARGET:-$(rustc -vV | sed -n 's/^host: //p')}"
VOICE_EXECUTABLE="$ROOT/src-tauri/binaries/sikemux-voice-$VOICE_TARGET"
[[ -x "$VOICE_EXECUTABLE" ]] || fail "voice helper is missing or not executable"
[[ ! -e "$APP_PATH/Contents/MacOS/sikemux-voice" ]] || fail "the voice helper is bundled in the app"
/usr/bin/codesign --verify --strict "$VOICE_EXECUTABLE" || fail "voice helper signature is invalid"
VOICE_SIGNATURE="$(/usr/bin/codesign -dv "$VOICE_EXECUTABLE" 2>&1)"
grep -q 'flags=.*runtime' <<<"$VOICE_SIGNATURE" || fail "voice helper lacks the hardened runtime"
VOICE_ARCHS="$(/usr/bin/lipo -archs "$VOICE_EXECUTABLE")"
sorted_archs() { tr ' ' '\n' <<<"$1" | sort | tr '\n' ' '; }
[[ "$(sorted_archs "$VOICE_ARCHS")" == "$(sorted_archs "$ARCHS")" ]] || fail "voice helper architecture ($VOICE_ARCHS) differs from app ($ARCHS)"
NOTCH_APP="$APP_PATH/Contents/Helpers/Sikemux Notch.app"
NOTCH_EXECUTABLE="$NOTCH_APP/Contents/MacOS/sikemux-notch"
[[ -x "$NOTCH_EXECUTABLE" ]] || fail "notch helper is missing or not executable"
/usr/bin/codesign --verify --strict "$NOTCH_APP" || fail "notch helper signature is invalid"
NOTCH_ARCHS="$(/usr/bin/lipo -archs "$NOTCH_EXECUTABLE")"
[[ "$(sorted_archs "$NOTCH_ARCHS")" == "$(sorted_archs "$ARCHS")" ]] || fail "notch helper architecture ($NOTCH_ARCHS) differs from app ($ARCHS)"
# The simulator helper is published and downloaded the same way.
SIM_EXECUTABLE="$ROOT/src-tauri/binaries/sikemux-sim-$VOICE_TARGET"
[[ -x "$SIM_EXECUTABLE" ]] || fail "simulator helper is missing or not executable"
[[ ! -e "$APP_PATH/Contents/MacOS/sikemux-sim" ]] || fail "the simulator helper is bundled in the app"
/usr/bin/codesign --verify --strict "$SIM_EXECUTABLE" || fail "simulator helper signature is invalid"
grep -q 'flags=.*runtime' <<<"$(/usr/bin/codesign -dv "$SIM_EXECUTABLE" 2>&1)" || fail "simulator helper lacks the hardened runtime"
SIM_ARCHS="$(/usr/bin/lipo -archs "$SIM_EXECUTABLE")"
[[ "$(sorted_archs "$SIM_ARCHS")" == "$(sorted_archs "$ARCHS")" ]] || fail "simulator helper architecture ($SIM_ARCHS) differs from app ($ARCHS)"

# Packaged apps must never depend on libraries from the build machine's
# Homebrew/MacPorts installation. Such binaries pass codesign verification but
# fail at launch on user machines (or under library-validation Team ID checks).
DYNAMIC_LIBS="$(/usr/bin/otool -L "$EXECUTABLE")"
if grep -Eq '^[[:space:]]+(/opt/homebrew|/usr/local|/opt/local)/' <<<"$DYNAMIC_LIBS"; then
  echo "$DYNAMIC_LIBS" >&2
  fail "bundle executable links to a package-manager library"
fi
CLI_DYNAMIC_LIBS="$(/usr/bin/otool -L "$CLI_EXECUTABLE")"
if grep -Eq '^[[:space:]]+(/opt/homebrew|/usr/local|/opt/local)/' <<<"$CLI_DYNAMIC_LIBS"; then
  echo "$CLI_DYNAMIC_LIBS" >&2
  fail "the core links to a package-manager library"
fi
VOICE_DYNAMIC_LIBS="$(/usr/bin/otool -L "$VOICE_EXECUTABLE")"
if grep -Eq '^[[:space:]]+(/opt/homebrew|/usr/local|/opt/local)/' <<<"$VOICE_DYNAMIC_LIBS"; then
  echo "$VOICE_DYNAMIC_LIBS" >&2
  fail "voice helper links to a package-manager library"
fi
SIM_DYNAMIC_LIBS="$(/usr/bin/otool -L "$SIM_EXECUTABLE")"
if grep -Eq '^[[:space:]]+(/opt/homebrew|/usr/local|/opt/local)/' <<<"$SIM_DYNAMIC_LIBS"; then
  echo "$SIM_DYNAMIC_LIBS" >&2
  fail "simulator helper links to a package-manager library"
fi

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

# The signed voice helper must still start under the hardened runtime.
if [[ "$VOICE_ARCHS" == *"$(uname -m)"* ]]; then
  "$VOICE_EXECUTABLE" --version | grep -Fq "sikemux-voice" || fail "signed voice helper does not start"
fi
if [[ "$SIM_ARCHS" == *"$(uname -m)"* ]]; then
  "$SIM_EXECUTABLE" --version | grep -Fq "sikemux-sim" || fail "signed simulator helper does not start"
fi

# A release requires a structurally valid ad hoc signature before it signs the app again.
if /usr/bin/codesign --verify --deep --strict --verbose=2 "$APP_PATH" 2>/dev/null; then
  echo "  ✓ code signature is structurally valid"
elif [[ "${REQUIRE_VALID_SIGNATURE:-0}" == "1" ]]; then
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
echo "  voice helper: $VOICE_EXECUTABLE ($VOICE_ARCHS)"
echo "  simulator helper: $SIM_EXECUTABLE ($SIM_ARCHS)"
