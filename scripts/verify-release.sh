#!/usr/bin/env bash
# Verify the release scripts/sign-mac.sh signed, start what it signed, and write
# the update manifest. It needs no signing material, so the Release workflow runs
# it on a machine that holds none.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd -P)"
VERSION="${1:-}"
NOTES="${2:-}"
CHANNEL=stable
[[ "${3:-}" == "--nightly" ]] && CHANNEL=nightly
NOTARIZED="${RELEASE_NOTARIZED:-0}"

fail() {
  echo "Release verification failed: $*" >&2
  exit 1
}

[[ -n "$VERSION" && -n "$NOTES" ]] || { echo "Usage: $0 <version> <notes> [--nightly]" >&2; exit 2; }
[[ "$NOTARIZED" == "0" || "$NOTARIZED" == "1" ]] || fail "RELEASE_NOTARIZED must be 0 or 1"

BUNDLE="$ROOT/target/release/bundle"
APP_NAME="$(node scripts/app-metadata.mjs name)"
APP_IDENTIFIER="$(node scripts/app-metadata.mjs identifier)"
TAR="$BUNDLE/macos/${APP_NAME}.app.tar.gz"
SIG="$TAR.sig"
APP="$BUNDLE/macos/${APP_NAME}.app"
shopt -s nullglob
DMG_CANDIDATES=("$BUNDLE"/dmg/*.dmg)
VOICES=("$ROOT"/packaging/binaries/sikemux-voice-*)
SIMS=("$ROOT"/packaging/binaries/sikemux-sim-*)
shopt -u nullglob
[[ ${#DMG_CANDIDATES[@]} -eq 1 ]] || fail "expected exactly one DMG, found ${#DMG_CANDIDATES[@]}"
[[ ${#VOICES[@]} -eq 1 ]] || fail "expected exactly one voice helper, found ${#VOICES[@]}"
[[ ${#SIMS[@]} -eq 1 ]] || fail "expected exactly one simulator helper, found ${#SIMS[@]}"
DMG="${DMG_CANDIDATES[0]}"
VOICE="${VOICES[0]}"
SIM="${SIMS[0]}"

EXTRACTED=""
DMG_MOUNT=""
DMG_ATTACHED=0
cleanup() {
  [[ "$DMG_ATTACHED" == "1" ]] && /usr/bin/hdiutil detach "$DMG_MOUNT" -quiet >/dev/null 2>&1
  [[ -n "$EXTRACTED" ]] && rm -rf "$EXTRACTED"
  [[ -n "$DMG_MOUNT" ]] && rm -rf "$DMG_MOUNT"
  return 0
}
trap cleanup EXIT

signing_info() {
  /usr/bin/codesign -dv --verbose=4 "$1" 2>&1
}
team_of() {
  signing_info "$1" | /usr/bin/sed -n 's/^TeamIdentifier=//p'
}
cdhash_of() {
  signing_info "$1" | /usr/bin/sed -n 's/^CDHash=//p'
}

[[ -d "$APP" ]] || fail "app bundle was not produced"
if [[ "$NOTARIZED" == "1" ]]; then
  /usr/sbin/spctl --assess --type execute --verbose=4 "$APP" || fail "app failed Gatekeeper assessment"
  /usr/bin/xcrun stapler validate "$APP" || fail "app has no valid notarization ticket"
  grep -q '^Authority=' <<<"$(signing_info "$APP")" || fail "release app has no certificate authority (ad-hoc signature)"
  APP_TEAM="$(team_of "$APP")"
  [[ -n "$APP_TEAM" && "$APP_TEAM" != "not set" ]] || fail "release app has no TeamIdentifier"
fi
[[ -s "$TAR" ]] || fail "updater archive was not produced"
[[ -s "$SIG" ]] || fail "updater signature was not produced"
[[ -s "$DMG" ]] || fail "DMG is empty"
/usr/bin/hdiutil verify "$DMG" >/dev/null || fail "DMG verification failed"
node "$ROOT/scripts/verify-updater-signature.mjs" "$TAR" "$SIG"
grep -q "^trusted comment: .*"$'\t'"version:$VERSION\$" <<<"$(/usr/bin/base64 -d -i "$SIG")" ||
  fail "updater signature is not bound to version $VERSION"
UPDATE_BUDGET_BYTES=$((32 * 1024 * 1024))
TAR_BYTES="$(stat -f %z "$TAR")"
echo "→ Updater archive is $TAR_BYTES bytes (budget $UPDATE_BUDGET_BYTES)"
((TAR_BYTES <= UPDATE_BUDGET_BYTES)) || fail "updater archive is $TAR_BYTES bytes, over the $UPDATE_BUDGET_BYTES byte budget"
if grep -Eqv "^${APP_NAME}\.app(/|\$)" <<<"$(/usr/bin/tar -tzf "$TAR")"; then
  fail "updater archive holds more than ${APP_NAME}.app"
fi

EXTRACTED="$(mktemp -d)"
/usr/bin/tar -xzf "$TAR" -C "$EXTRACTED"
ARCHIVE_APP="$EXTRACTED/${APP_NAME}.app"
[[ -d "$ARCHIVE_APP" ]] || fail "updater archive does not contain ${APP_NAME}.app"
/usr/bin/codesign --verify --deep --strict --verbose=2 "$ARCHIVE_APP" || fail "archived app code signature is invalid"
if [[ "$NOTARIZED" == "1" ]]; then
  /usr/sbin/spctl --assess --type execute --verbose=4 "$ARCHIVE_APP" || fail "updater app failed Gatekeeper assessment"
  /usr/bin/xcrun stapler validate "$ARCHIVE_APP" || fail "updater app has no valid notarization ticket"
fi
ARCHIVE_PLIST="$ARCHIVE_APP/Contents/Info.plist"
[[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$ARCHIVE_PLIST")" == "$VERSION" ]] || fail "archived app version is wrong"
[[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIconName' "$ARCHIVE_PLIST")" == "sikemux" ]] || fail "archived app icon metadata is wrong"
EXECUTABLE_NAME="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$ARCHIVE_PLIST")"
ARCHS="$(/usr/bin/lipo -archs "$ARCHIVE_APP/Contents/MacOS/$EXECUTABLE_NAME")"
APP_CDHASH="$(cdhash_of "$APP")"
[[ -n "$APP_CDHASH" && "$(cdhash_of "$ARCHIVE_APP")" == "$APP_CDHASH" ]] || fail "updater archive does not contain the exact signed app"

CORE_APP="$ARCHIVE_APP/Contents/Helpers/Sikemux Core.app"
CLI_EXECUTABLE="$CORE_APP/Contents/MacOS/sikemux-host"
[[ "$(readlink "$ARCHIVE_APP/Contents/MacOS/sikemux-host")" == "../Helpers/Sikemux Core.app/Contents/MacOS/sikemux-host" ]] ||
  fail "Contents/MacOS/sikemux-host does not link to the bundled core"
[[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$CORE_APP/Contents/Info.plist")" == "$APP_IDENTIFIER.core" ]] ||
  fail "the core's CFBundleIdentifier is not $APP_IDENTIFIER.core"
grep -qx "Identifier=$APP_IDENTIFIER.core" <<<"$(signing_info "$CORE_APP")" || fail "the core is not signed as $APP_IDENTIFIER.core"

# The app and its core keep secrets in a keychain group only they may use,
# which macOS allows only with the profile that names them.
TEAM=D577WD6Z5U
check_keychain_entitlements() {
  local bundle="$1" kind="$2" profile="$ROOT/packaging/macos/$3" name signed decoded application now
  name="$(basename "$bundle")"
  signed="$EXTRACTED/$kind.entitlements"
  decoded="$EXTRACTED/$kind.provisionprofile.plist"
  grep -q 'flags=.*runtime' <<<"$(signing_info "$bundle")" || fail "$name lacks the hardened runtime"
  /usr/bin/cmp -s "$bundle/Contents/embedded.provisionprofile" "$profile" ||
    fail "$name does not embed $(basename "$profile")"
  /usr/bin/codesign -d --entitlements - --xml "$bundle" 2>/dev/null | /usr/bin/plutil -convert xml1 -o "$signed" - ||
    fail "could not read the entitlements $name is signed with"
  /usr/bin/cmp -s "$signed" <("$ROOT/scripts/mac-entitlements.sh" "$kind" | /usr/bin/plutil -convert xml1 -o - -) ||
    fail "$name is not signed with the entitlements scripts/mac-entitlements.sh $kind prints"
  [[ "$(/usr/bin/plutil -extract keychain-access-groups json -o - "$signed")" == "[\"$TEAM.$APP_IDENTIFIER\"]" ]] ||
    fail "$name's keychain group is not exactly $TEAM.$APP_IDENTIFIER"
  /usr/bin/security cms -D -i "$profile" -o "$decoded" 2>/dev/null || fail "could not read $(basename "$profile")"
  application="$(/usr/libexec/PlistBuddy -c 'Print :com.apple.application-identifier' "$signed")"
  [[ "$(/usr/libexec/PlistBuddy -c 'Print :Entitlements:com.apple.application-identifier' "$decoded")" == "$application" ]] ||
    fail "$(basename "$profile") does not authorize $application"
  [[ "$(/usr/libexec/PlistBuddy -c 'Print :TeamIdentifier:0' "$decoded")" == "$TEAM" ]] ||
    fail "$(basename "$profile") belongs to another team than $TEAM"
  now="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  [[ "$(/usr/bin/plutil -extract ExpirationDate raw -o - "$decoded")" > "$now" ]] || fail "$(basename "$profile") has expired"
}
check_keychain_entitlements "$ARCHIVE_APP" app sikemux-developer-id.provisionprofile
check_keychain_entitlements "$CORE_APP" core sikemux-core-developer-id.provisionprofile
NOTCH_APP="$ARCHIVE_APP/Contents/Helpers/Sikemux Notch.app"
grep -q 'flags=.*runtime' <<<"$(signing_info "$NOTCH_APP")" || fail "notch helper lacks the hardened runtime"

# The app downloads the voice and simulator helpers from this release and refuses
# any file but the one it was built with, so the published helpers must carry
# those hashes.
for helper in "$VOICE" "$SIM"; do
  name="$(basename "$helper")"
  /usr/bin/codesign --verify --strict "$helper" || fail "$name signature is invalid"
  grep -q 'flags=.*runtime' <<<"$(signing_info "$helper")" || fail "$name lacks the hardened runtime"
  if [[ "$NOTARIZED" == "1" ]]; then
    grep -q '^Authority=' <<<"$(signing_info "$helper")" || fail "release $name has no certificate authority (ad-hoc signature)"
    [[ "$(team_of "$helper")" == "$APP_TEAM" ]] || fail "$name is signed by another team than the app"
  fi
  sha="$(/usr/bin/shasum -a 256 "$helper" | cut -d ' ' -f 1)"
  /usr/bin/grep -aqF "$sha" "$ARCHIVE_APP/Contents/MacOS/$EXECUTABLE_NAME" ||
    fail "the app does not expect the $name about to be published"
done

# Only starting the signed copies proves they survive the hardened runtime, and
# that macOS accepts the profile behind the core's keychain group. It kills a
# core signed ad hoc at once, since nothing vouches for that group. An empty
# agent id is the earliest thing the core's tools MCP server checks.
if [[ "$NOTARIZED" == "1" ]]; then
  TOOLS_START="$(SIKEMUX_TOOLS_AGENT_ID='' "$CLI_EXECUTABLE" --tools-mcp 2>&1 || true)"
  if ! grep -Fq "Missing SIKEMUX_TOOLS_AGENT_ID" <<<"$TOOLS_START"; then
    echo "$TOOLS_START" >&2
    fail "signed tools MCP server does not start"
  fi
fi
if [[ "$(/usr/bin/lipo -archs "$VOICE")" == *"$(uname -m)"* ]]; then
  "$VOICE" --version | grep -Fq "sikemux-voice" || fail "signed voice helper does not start"
fi
if [[ "$(/usr/bin/lipo -archs "$SIM")" == *"$(uname -m)"* ]]; then
  "$SIM" --version | grep -Fq "sikemux-sim" || fail "signed simulator helper does not start"
fi

DMG_MOUNT="$(mktemp -d)"
/usr/bin/hdiutil attach -nobrowse -readonly -mountpoint "$DMG_MOUNT" "$DMG" >/dev/null || fail "could not mount DMG"
DMG_ATTACHED=1
DMG_APP="$DMG_MOUNT/${APP_NAME}.app"
[[ -d "$DMG_APP" ]] || fail "DMG does not contain ${APP_NAME}.app at its root"
/usr/bin/codesign --verify --deep --strict --verbose=2 "$DMG_APP" || fail "DMG app code signature is invalid"
if [[ "$NOTARIZED" == "1" ]]; then
  /usr/sbin/spctl --assess --type execute --verbose=4 "$DMG_APP" || fail "DMG app failed Gatekeeper assessment"
  /usr/sbin/spctl --assess --type open --context context:primary-signature --verbose=4 "$DMG" || fail "DMG failed Gatekeeper assessment"
  /usr/bin/xcrun stapler validate "$DMG" || fail "DMG has no valid notarization ticket"
fi
[[ "$(cdhash_of "$DMG_APP")" == "$APP_CDHASH" ]] || fail "DMG does not contain the exact signed app"
DMG_PLIST="$DMG_APP/Contents/Info.plist"
[[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$DMG_PLIST")" == "$VERSION" ]] || fail "DMG app version is wrong"
[[ "$(/usr/bin/lipo -archs "$DMG_APP/Contents/MacOS/$EXECUTABLE_NAME")" == "$ARCHS" ]] || fail "DMG app architectures differ from updater app"
/usr/bin/hdiutil detach "$DMG_MOUNT" -quiet
DMG_ATTACHED=0
# The README and sikemux.com promise a small download.
DMG_BUDGET_BYTES=25000000
DMG_BYTES="$(stat -f %z "$DMG")"
((DMG_BYTES <= DMG_BUDGET_BYTES)) || fail "$DMG is $DMG_BYTES bytes, over the $DMG_BUDGET_BYTES byte budget"

PLATFORMS=()
[[ " $ARCHS " == *" arm64 "* ]] && PLATFORMS+=(darwin-aarch64)
[[ " $ARCHS " == *" x86_64 "* ]] && PLATFORMS+=(darwin-x86_64)
[[ ${#PLATFORMS[@]} -gt 0 ]] || fail "unsupported updater architecture set: $ARCHS"
for arch in $ARCHS; do
  [[ "$arch" == "arm64" || "$arch" == "x86_64" ]] || fail "unsupported executable architecture: $arch"
done

PUB_DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
# Artifacts live on a tag that never moves. The nightly release carries only
# latest.json, because shipped clients resolve that URL and cannot be repointed.
RELEASE_TAG="v$VERSION"
TAR_URL="https://github.com/nodelike/sikemux/releases/download/$RELEASE_TAG/${APP_NAME}.app.tar.gz"
# Only a stable cut owns the tracked manifest. A nightly one lands beside its
# artifacts, which git ignores, so it cannot overwrite the stable feed.
if [[ "$CHANNEL" == "stable" ]]; then
  MANIFEST="$ROOT/latest.json"
else
  MANIFEST="$BUNDLE/latest.json"
fi
# Credits are a nicety. Without them the What's new modal asks GitHub itself.
CREDITS="$(node scripts/release-credits.mjs "$VERSION" "$(git rev-parse HEAD)")" || {
  echo "! Could not gather release credits; latest.json ships without them." >&2
  CREDITS=""
}
PLATFORM_LIST="${PLATFORMS[*]}" VERSION="$VERSION" NOTES="$NOTES" PUB_DATE="$PUB_DATE" SIG="$SIG" TAR_URL="$TAR_URL" MANIFEST="$MANIFEST" CREDITS="$CREDITS" python3 - <<'PY'
import json, os, pathlib
entry = {
    "signature": pathlib.Path(os.environ["SIG"]).read_text().strip(),
    "url": os.environ["TAR_URL"],
}
manifest = {
    "version": os.environ["VERSION"],
    "notes": os.environ["NOTES"],
    "pub_date": os.environ["PUB_DATE"],
    "platforms": {platform: dict(entry) for platform in os.environ["PLATFORM_LIST"].split()},
}
if os.environ["CREDITS"]:
    manifest["credits"] = json.loads(os.environ["CREDITS"])
pathlib.Path(os.environ["MANIFEST"]).write_text(json.dumps(manifest, indent=2) + "\n")
PY
python3 -m json.tool "$MANIFEST" >/dev/null

echo "✓ Verified $CHANNEL release v$VERSION ($ARCHS)"
echo "  $DMG"
echo "  $TAR"
echo "  $SIG"
echo "  $VOICE"
echo "  $SIM"
echo "  $MANIFEST"
