#!/usr/bin/env bash
# Build and validate the macOS app bundle, ad hoc signed. Tauri merges
# src-tauri/Info.plist before signing; this script never mutates the completed
# .app. A release signs it afterwards with scripts/sign-mac.sh, on a machine
# that never runs the build's dependencies.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd -P)"
APP_NAME="$(node -p "require('./src-tauri/tauri.conf.json').productName")"
APP_VERSION="$(node -p "require('./src-tauri/tauri.conf.json').version")"
APP_IDENTIFIER="$(node -p "require('./src-tauri/tauri.conf.json').identifier")"
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

BUILD_ARGS=("$@")
BUILD_ARGS+=(--config "$ROOT/src-tauri/tauri.core.conf.json")
BUILD_ARGS+=(--config "$ROOT/src-tauri/tauri.notch.conf.json")
BUILD_ARGS+=(--config '{"bundle":{"createUpdaterArtifacts":false}}')
# No signing material reaches the build. The bundler would notarize whenever
# Apple's credentials are set, even to empty strings.
unset TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD
unset APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID APPLE_API_KEY APPLE_API_ISSUER APPLE_API_KEY_PATH
export APPLE_SIGNING_IDENTITY=-

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
# A release builds the voice and simulator helpers first and signs them, because
# the app is compiled with their hashes.
if [[ "${PREBUILT_HELPERS:-0}" != "1" && "$STEP" != "package" ]]; then
  node "$ROOT/scripts/build-voice-helper.mjs" ${TARGET_ARGS[@]+"${TARGET_ARGS[@]}"}
  node "$ROOT/scripts/build-sim-helper.mjs" ${TARGET_ARGS[@]+"${TARGET_ARGS[@]}"}
fi
if [[ "$STEP" != "package" ]]; then
  printf '→ pnpm tauri build --no-bundle'
  printf ' %q' "${BUILD_ARGS[@]}"
  echo
  pnpm tauri build --no-bundle "${BUILD_ARGS[@]}"
fi
[[ "$STEP" == "compile" ]] && exit 0
node "$ROOT/scripts/build-notch-helper.mjs" ${TARGET_ARGS[@]+"${TARGET_ARGS[@]}"}
node "$ROOT/scripts/build-cli-sidecar.mjs" ${TARGET_ARGS[@]+"${TARGET_ARGS[@]}"}
printf '→ pnpm tauri bundle'
printf ' %q' "${BUILD_ARGS[@]}"
echo
pnpm tauri bundle "${BUILD_ARGS[@]}"

TARGET_ROOT="$ROOT/src-tauri/target"
[[ -n "$TARGET" ]] && TARGET_ROOT="$TARGET_ROOT/$TARGET"
BUNDLE="$TARGET_ROOT/release/bundle"
APP_PATH="$BUNDLE/macos/${APP_NAME}.app"
PLIST="$APP_PATH/Contents/Info.plist"

fail() {
  echo "macOS bundle verification failed: $*" >&2
  exit 1
}

# The README and sikemux.com promise a small download, so a release that grows
# past this fails here instead of shipping. scripts/sign-mac.sh checks it again.
DMG_BUDGET_BYTES=20000000

# Tauri packs the DMG with zlib; LZMA makes it about a fifth smaller.
shopt -s nullglob
for DMG in "$BUNDLE"/dmg/*.dmg; do
  PACKED="${DMG%.dmg}.lzma.dmg"
  /usr/bin/hdiutil convert "$DMG" -format ULMO -o "$PACKED" -quiet -ov || fail "could not repack $DMG"
  mv -f "$PACKED" "$DMG"
  DMG_BYTES="$(stat -f%z "$DMG")"
  ((DMG_BYTES <= DMG_BUDGET_BYTES)) || fail "$DMG is $DMG_BYTES bytes, over the $DMG_BUDGET_BYTES byte budget"
done
shopt -u nullglob

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
