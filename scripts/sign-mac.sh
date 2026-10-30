#!/usr/bin/env bash
# Sign the app scripts/build-mac.sh built, notarize it, and pack the updater
# archive and the DMG around it. It runs only Apple's tools, this repository's
# scripts and a pinned Tauri CLI, never the build's dependencies, and never runs
# what it signs.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd -P)"
VERSION="${1:-}"
NOTARIZED="${RELEASE_NOTARIZED:-0}"
TAURI_CLI_VERSION=2.12.0

fail() {
  echo "Release signing failed: $*" >&2
  exit 1
}

[[ -n "$VERSION" ]] || { echo "Usage: $0 <version>" >&2; exit 2; }
[[ "$NOTARIZED" == "0" || "$NOTARIZED" == "1" ]] || fail "RELEASE_NOTARIZED must be 0 or 1"
[[ "$(uname -s)" == "Darwin" ]] || fail "macOS is required"
[[ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ]] || fail "TAURI_SIGNING_PRIVATE_KEY is not set"
if [[ "$NOTARIZED" == "1" ]]; then
  IDENTITY="${APPLE_SIGNING_IDENTITY:-}"
  [[ -n "$IDENTITY" ]] || fail "APPLE_SIGNING_IDENTITY is not set"
  /usr/bin/security find-identity -v -p codesigning | grep -Fq "\"$IDENTITY\"" ||
    fail "APPLE_SIGNING_IDENTITY is not available in the keychain"
  if [[ -n "${APPLE_API_KEY:-}" && -n "${APPLE_API_ISSUER:-}" && -n "${APPLE_API_KEY_PATH:-}" ]]; then
    NOTARY=(--key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY" --issuer "$APPLE_API_ISSUER")
  elif [[ -n "${APPLE_ID:-}" && -n "${APPLE_PASSWORD:-}" && -n "${APPLE_TEAM_ID:-}" ]]; then
    NOTARY=(--apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID")
  else
    fail "Apple notarization credentials are not configured"
  fi
  TIMESTAMP=(--timestamp)
else
  IDENTITY=-
  TIMESTAMP=()
fi
# Each tool below gets only the secret it needs.
UPDATER_KEY="$TAURI_SIGNING_PRIVATE_KEY"
UPDATER_KEY_PASSWORD="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}"
unset TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD
unset APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID APPLE_API_KEY APPLE_API_ISSUER APPLE_API_KEY_PATH

APP_NAME="$(node scripts/app-metadata.mjs name)"
APP_IDENTIFIER="$(node scripts/app-metadata.mjs identifier)"
BUNDLE="$ROOT/target/release/bundle"
APP="$BUNDLE/macos/${APP_NAME}.app"
TAR="$BUNDLE/macos/${APP_NAME}.app.tar.gz"
SIG="$TAR.sig"
NOTCH_APP="$APP/Contents/Helpers/Sikemux Notch.app"
CORE_APP="$APP/Contents/Helpers/Sikemux Core.app"
CORE_LINK="$APP/Contents/MacOS/sikemux-host"
CORE_LINK_TARGET="../Helpers/Sikemux Core.app/Contents/MacOS/sikemux-host"

WORK="$(mktemp -d)"
MOUNT=""
cleanup() {
  [[ -n "$MOUNT" ]] && /usr/bin/hdiutil detach "$MOUNT" -quiet >/dev/null 2>&1
  rm -rf "$WORK"
}
trap cleanup EXIT

# The app came from a machine that ran the dependencies. A link could point
# codesign or ditto at files outside it, and code this script does not know
# would ship signed without anyone choosing to. The one link allowed leads to
# the core, where installed `sikemux` commands point.
[[ -d "$APP" ]] || fail "missing $APP"
[[ -L "$CORE_LINK" && "$(readlink "$CORE_LINK")" == "$CORE_LINK_TARGET" ]] ||
  fail "Contents/MacOS/sikemux-host does not link to the bundled core"
[[ -z "$(find "$APP" ! -type f ! -type d ! -path "$CORE_LINK" -print -quit)" ]] ||
  fail "$APP holds something other than files, folders and the link to its core"
EXECUTABLE_NAME="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$APP/Contents/Info.plist")"
expected_code="$(printf '%s\n' "Contents/Helpers/Sikemux Notch.app/Contents/MacOS/sikemux-notch" \
  "Contents/MacOS/$EXECUTABLE_NAME" "Contents/Helpers/Sikemux Core.app/Contents/MacOS/sikemux-host" | sort)"
found_code="$(cd "$APP" && find . -type f -print0 | while IFS= read -r -d '' file; do
  if /usr/bin/file -b "$file" | grep -q '^Mach-O'; then echo "${file#./}"; fi
done | sort)"
[[ "$found_code" == "$expected_code" ]] || fail "$APP carries other code than the app, its core and the notch helper: $found_code"
[[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$APP/Contents/Info.plist")" == "$APP_IDENTIFIER" ]] || fail "app identifier is wrong"
[[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$CORE_APP/Contents/Info.plist")" == "$APP_IDENTIFIER.core" ]] || fail "core identifier is wrong"
[[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$CORE_APP/Contents/Info.plist")" == "sikemux-host" ]] || fail "core executable is wrong"
[[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist")" == "$VERSION" ]] || fail "app version is not $VERSION"
shopt -s nullglob
DMGS=("$BUNDLE"/dmg/*.dmg)
shopt -u nullglob
[[ ${#DMGS[@]} -eq 1 ]] || fail "expected exactly one DMG, found ${#DMGS[@]}"
DMG="${DMGS[0]}"

echo "→ Fetching Tauri CLI $TAURI_CLI_VERSION"
case "$(uname -m)" in
  arm64) TRIPLE=aarch64-apple-darwin TAURI_CLI_SHA256=c0d45bb76b9501ee8871a1712bc8e45d0b6a97d0695e0de85b4477794f07682a ;;
  x86_64) TRIPLE=x86_64-apple-darwin TAURI_CLI_SHA256=157b919fa24c6c14ff34f5ae4046b8df59c894d1955b845f01a9a30d07abe8b9 ;;
  *) fail "no pinned Tauri CLI for $(uname -m)" ;;
esac
/usr/bin/curl -fsSL --retry 3 -o "$WORK/tauri-cli.zip" \
  "https://github.com/tauri-apps/tauri/releases/download/tauri-cli-v$TAURI_CLI_VERSION/cargo-tauri-$TRIPLE.zip"
[[ "$(/usr/bin/shasum -a 256 "$WORK/tauri-cli.zip" | cut -d ' ' -f 1)" == "$TAURI_CLI_SHA256" ]] ||
  fail "the Tauri CLI download does not match its pinned hash"
/usr/bin/ditto -x -k "$WORK/tauri-cli.zip" "$WORK/tauri-cli"
TAURI_CLI="$WORK/tauri-cli/cargo-tauri"

notarize() {
  local status
  echo "→ Notarizing $(basename "$1")"
  status="$(/usr/bin/xcrun notarytool submit "$1" "${NOTARY[@]}" --wait --output-format json |
    node -e 'process.stdout.write(JSON.parse(require("fs").readFileSync(0, "utf8")).status ?? "")')" ||
    fail "could not submit $1 for notarization"
  [[ "$status" == "Accepted" ]] || fail "Apple did not accept $1 for notarization: $status"
}

echo "→ Signing $APP_NAME.app as $IDENTITY"
# The profiles let the app and its core claim the keychain group they share.
/usr/bin/ditto "$ROOT/packaging/macos/sikemux-developer-id.provisionprofile" "$APP/Contents/embedded.provisionprofile"
/usr/bin/ditto "$ROOT/packaging/macos/sikemux-core-developer-id.provisionprofile" "$CORE_APP/Contents/embedded.provisionprofile"
"$ROOT/scripts/mac-entitlements.sh" app >"$WORK/app.entitlements"
"$ROOT/scripts/mac-entitlements.sh" core >"$WORK/core.entitlements"
/usr/bin/codesign --force --options runtime ${TIMESTAMP[@]+"${TIMESTAMP[@]}"} --sign "$IDENTITY" "$NOTCH_APP"
/usr/bin/codesign --force --options runtime ${TIMESTAMP[@]+"${TIMESTAMP[@]}"} \
  --entitlements "$WORK/core.entitlements" --sign "$IDENTITY" "$CORE_APP"
/usr/bin/codesign --force --options runtime ${TIMESTAMP[@]+"${TIMESTAMP[@]}"} \
  --entitlements "$WORK/app.entitlements" --sign "$IDENTITY" "$APP"
/usr/bin/codesign --verify --deep --strict --verbose=2 "$APP" || fail "app signature is invalid"
if [[ "$NOTARIZED" == "1" ]]; then
  /usr/bin/ditto -c -k --keepParent "$APP" "$WORK/app.zip"
  notarize "$WORK/app.zip"
  /usr/bin/xcrun stapler staple "$APP" || fail "could not staple the notarization ticket to the app"
fi

echo "→ Packing and signing the updater archive"
rm -f "$TAR" "$SIG"
# The updater drops the first path component of every entry, so the archive
# holds the app alone, without macOS metadata entries beside it.
COPYFILE_DISABLE=1 /usr/bin/tar --no-mac-metadata --no-xattrs --no-acls --no-fflags \
  -czf "$TAR" -C "$(dirname "$APP")" "$(basename "$APP")"
TAURI_SIGNING_PRIVATE_KEY="$UPDATER_KEY" TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$UPDATER_KEY_PASSWORD" \
  "$TAURI_CLI" signer sign --app-version "$VERSION" "$TAR" >/dev/null
node "$ROOT/scripts/verify-updater-signature.mjs" "$TAR" "$SIG"

echo "→ Putting the signed app in $(basename "$DMG")"
/usr/bin/hdiutil convert "$DMG" -format UDRW -o "$WORK/layout.dmg" -quiet || fail "could not unpack $DMG"
MOUNT="$WORK/volume"
mkdir "$MOUNT"
/usr/bin/hdiutil attach "$WORK/layout.dmg" -readwrite -nobrowse -noautoopen -mountpoint "$MOUNT" -quiet ||
  fail "could not mount $DMG"
shopt -s nullglob dotglob
for entry in "$MOUNT"/*; do
  case "$(basename "$entry")" in
    "$APP_NAME.app" | .background | .VolumeIcon.icns | .DS_Store | .fseventsd | .Trashes) ;;
    Applications) [[ -L "$entry" && "$(readlink "$entry")" == /Applications ]] || fail "the DMG's Applications is not a link to /Applications" ;;
    *) fail "the DMG holds an unexpected $(basename "$entry")" ;;
  esac
done
shopt -u nullglob dotglob
[[ -d "$MOUNT/$APP_NAME.app" && ! -L "$MOUNT/$APP_NAME.app" ]] || fail "the DMG does not hold $APP_NAME.app"
if [[ "$(ls -A "$MOUNT/.background")" != "background.tiff" ]] ||
  ! /usr/bin/cmp -s "$MOUNT/.background/background.tiff" "$ROOT/packaging/dmg/background.tiff"; then
  fail "the DMG background is not packaging/dmg/background.tiff"
fi
rm -rf "${MOUNT:?}/$APP_NAME.app" "$MOUNT/.fseventsd" "$MOUNT/.Trashes"
/usr/bin/ditto "$APP" "$MOUNT/$APP_NAME.app"
/usr/bin/hdiutil detach "$MOUNT" -quiet || fail "could not unmount the DMG"
MOUNT=""
# Converting copies only the space files use, so the replaced app leaves nothing behind.
/usr/bin/hdiutil convert "$WORK/layout.dmg" -format ULMO -o "$WORK/packed.dmg" -quiet || fail "could not repack $DMG"
mv -f "$WORK/packed.dmg" "$DMG"
if [[ "$NOTARIZED" == "1" ]]; then
  /usr/bin/codesign --force --timestamp --sign "$IDENTITY" "$DMG" || fail "could not sign $DMG"
  notarize "$DMG"
  /usr/bin/xcrun stapler staple "$DMG" || fail "could not staple the notarization ticket to $DMG"
fi

echo "✓ Signed $APP_NAME $VERSION"
echo "  $APP"
echo "  $DMG"
echo "  $TAR"
echo "  $SIG"
