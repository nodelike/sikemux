#!/usr/bin/env bash
# Build and validate the macOS app bundle. Tauri merges src-tauri/Info.plist
# before signing; this script never mutates the completed .app.
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
BUILD_ARGS+=(--config "$ROOT/src-tauri/tauri.sidecar.conf.json")
BUILD_ARGS+=(--config "$ROOT/src-tauri/tauri.notch.conf.json")
# Normal developer builds do not have the updater private key, so avoid asking
# Tauri to create an updater archive it cannot sign.
if [[ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ]]; then
  BUILD_ARGS+=(--config '{"bundle":{"createUpdaterArtifacts":false}}')
fi
# Only the bundling step signs the updater archive, so the compilers, build
# scripts and frontend tooling never see the key in their environment.
UPDATER_KEY="${TAURI_SIGNING_PRIVATE_KEY:-}"
UPDATER_KEY_PASSWORD="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}"
unset TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD
# Community releases and local builds use a complete ad-hoc bundle signature.
# RELEASE_NOTARIZED=1 supplies a real Developer ID identity instead.
if [[ "${REQUIRE_SIGNED_APP:-0}" != "1" && -z "${APPLE_SIGNING_IDENTITY:-}" ]]; then
  export APPLE_SIGNING_IDENTITY="${APPLE_SIGNING_IDENTITY:--}"
fi

"$ROOT/scripts/icons.sh"
if [[ -n "$TARGET" ]]; then
  node "$ROOT/scripts/build-cli-sidecar.mjs" --target "$TARGET"
  node "$ROOT/scripts/build-voice-helper.mjs" --target "$TARGET"
  node "$ROOT/scripts/build-notch-helper.mjs" --target "$TARGET"
  node "$ROOT/scripts/build-sim-helper.mjs" --target "$TARGET"
else
  node "$ROOT/scripts/build-cli-sidecar.mjs"
  node "$ROOT/scripts/build-voice-helper.mjs"
  node "$ROOT/scripts/build-notch-helper.mjs"
  node "$ROOT/scripts/build-sim-helper.mjs"
fi
printf '→ pnpm tauri build --no-bundle'
printf ' %q' "${BUILD_ARGS[@]}"
echo
pnpm tauri build --no-bundle "${BUILD_ARGS[@]}"
printf '→ pnpm tauri bundle'
printf ' %q' "${BUILD_ARGS[@]}"
echo
if [[ -n "$UPDATER_KEY" ]]; then
  TAURI_SIGNING_PRIVATE_KEY="$UPDATER_KEY" TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$UPDATER_KEY_PASSWORD" \
    pnpm tauri bundle "${BUILD_ARGS[@]}"
else
  pnpm tauri bundle "${BUILD_ARGS[@]}"
fi

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
# past this fails here instead of shipping.
DMG_BUDGET_BYTES=15000000

# Tauri packs the DMG with zlib; LZMA makes it about a fifth smaller. The
# conversion drops the DMG's signature, so a real identity signs it again.
shopt -s nullglob
for DMG in "$BUNDLE"/dmg/*.dmg; do
  PACKED="${DMG%.dmg}.lzma.dmg"
  /usr/bin/hdiutil convert "$DMG" -format ULMO -o "$PACKED" -quiet -ov || fail "could not repack $DMG"
  mv -f "$PACKED" "$DMG"
  if [[ -n "${APPLE_SIGNING_IDENTITY:-}" && "$APPLE_SIGNING_IDENTITY" != "-" ]]; then
    /usr/bin/codesign --force --timestamp --sign "$APPLE_SIGNING_IDENTITY" "$DMG" || fail "could not sign $DMG"
  fi
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
CLI_EXECUTABLE="$APP_PATH/Contents/MacOS/sikemux-editor"
[[ -x "$CLI_EXECUTABLE" ]] || fail "bundled CLI sidecar is missing or not executable"
CLI_ARCHS="$(/usr/bin/lipo -archs "$CLI_EXECUTABLE")"
[[ "$CLI_ARCHS" == "$ARCHS" ]] || fail "CLI sidecar architecture ($CLI_ARCHS) differs from app ($ARCHS)"
[[ -s "$APP_PATH/Contents/Resources/sikemux_pi_tools.ts" ]] || fail "bundled Pi browser extension is missing"
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
  fail "CLI sidecar links to a package-manager library"
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

# Bundling is what gives the sidecar the hardened runtime, so only starting the
# bundled copy proves it survives signing: the copy built beside it is signed
# without the hardened runtime and starts whether or not the bundle would. An
# empty agent id is the earliest thing its tools MCP server checks.
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

# Every normal build is ad-hoc signed when no Apple identity is configured.
# Community releases require a structurally valid signature; notarized releases
# additionally require a real certificate authority and TeamIdentifier.
if /usr/bin/codesign --verify --deep --strict --verbose=2 "$APP_PATH" 2>/dev/null; then
  echo "  ✓ code signature is structurally valid"
elif [[ "${REQUIRE_VALID_SIGNATURE:-0}" == "1" || "${REQUIRE_SIGNED_APP:-0}" == "1" ]]; then
  /usr/bin/codesign --verify --deep --strict --verbose=2 "$APP_PATH" >&2 || true
  fail "codesign verification failed"
else
  echo "  ! local app is not fully code signed (release.sh enforces signing)" >&2
fi

if [[ "${REQUIRE_SIGNED_APP:-0}" == "1" ]]; then
  SIGNING_INFO="$(/usr/bin/codesign -dv --verbose=4 "$APP_PATH" 2>&1)"
  grep -q '^Authority=' <<<"$SIGNING_INFO" || fail "release app has no certificate authority (ad-hoc signature)"
  grep -q '^TeamIdentifier=' <<<"$SIGNING_INFO" || fail "release app has no TeamIdentifier"
  VOICE_SIGNING_INFO="$(/usr/bin/codesign -dv --verbose=4 "$VOICE_EXECUTABLE" 2>&1)"
  grep -q '^Authority=' <<<"$VOICE_SIGNING_INFO" || fail "release voice helper has no certificate authority (ad-hoc signature)"
  grep -q '^Authority=' <<<"$(/usr/bin/codesign -dv --verbose=4 "$SIM_EXECUTABLE" 2>&1)" || fail "release simulator helper has no certificate authority (ad-hoc signature)"
fi

echo ""
echo "✓ Verified macOS bundle"
echo "  app: $APP_PATH"
echo "  version: $APP_VERSION"
echo "  architectures: $ARCHS"
echo "  cli: $CLI_EXECUTABLE ($CLI_ARCHS)"
echo "  voice helper: $VOICE_EXECUTABLE ($VOICE_ARCHS)"
echo "  simulator helper: $SIM_EXECUTABLE ($SIM_ARCHS)"
