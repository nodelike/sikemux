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
# Normal developer builds do not have the updater private key, so avoid asking
# Tauri to create an updater archive it cannot sign.
if [[ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ]]; then
  BUILD_ARGS+=(--config '{"bundle":{"createUpdaterArtifacts":false}}')
fi
# Community releases and local builds use a complete ad-hoc bundle signature.
# RELEASE_NOTARIZED=1 supplies a real Developer ID identity instead.
if [[ "${REQUIRE_SIGNED_APP:-0}" != "1" && -z "${APPLE_SIGNING_IDENTITY:-}" ]]; then
  export APPLE_SIGNING_IDENTITY="${APPLE_SIGNING_IDENTITY:--}"
fi

"$ROOT/scripts/icons.sh"
if [[ -n "$TARGET" ]]; then
  node "$ROOT/scripts/build-cli-sidecar.mjs" --target "$TARGET"
else
  node "$ROOT/scripts/build-cli-sidecar.mjs"
fi
printf '→ pnpm tauri build'
if ((${#BUILD_ARGS[@]})); then
  printf ' %q' "${BUILD_ARGS[@]}"
  echo
  pnpm tauri build "${BUILD_ARGS[@]}"
else
  echo
  pnpm tauri build
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
fi

echo ""
echo "✓ Verified macOS bundle"
echo "  app: $APP_PATH"
echo "  version: $APP_VERSION"
echo "  architectures: $ARCHS"
echo "  cli: $CLI_EXECUTABLE ($CLI_ARCHS)"
