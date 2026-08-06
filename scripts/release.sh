#!/usr/bin/env bash
# Build, verify, and optionally publish a signed macOS release.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd -P)"
VERSION="${1:-}"
NOTES="${2:-}"
PUBLISH="${RELEASE_PUBLISH:-0}"
NOTARIZED="${RELEASE_NOTARIZED:-0}"
CHANNEL="${RELEASE_CHANNEL:-stable}"
for arg in "${@:3}"; do
  [[ "$arg" == "--publish" ]] && PUBLISH=1
  [[ "$arg" == "--preview" ]] && CHANNEL=preview
  [[ "$arg" != "--publish" && "$arg" != "--preview" ]] && { echo "Unknown option: $arg" >&2; exit 2; }
done

if [[ -z "$VERSION" ]]; then
  echo "Usage: $0 <version> <notes> [--preview] [--publish]" >&2
  exit 2
fi
if ! VERSION="$VERSION" node - <<'NODE'
const semver = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-((?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*))?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/;
process.exit(semver.test(process.env.VERSION) ? 0 : 1);
NODE
then
  echo "Invalid release version: $VERSION" >&2
  exit 2
fi
if [[ "$NOTARIZED" != "0" && "$NOTARIZED" != "1" ]]; then
  echo "RELEASE_NOTARIZED must be 0 or 1" >&2
  exit 2
fi
if [[ "$CHANNEL" != "stable" && "$CHANNEL" != "preview" ]]; then
  echo "RELEASE_CHANNEL must be stable or preview" >&2
  exit 2
fi
VERSION_WITHOUT_BUILD="${VERSION%%+*}"
if [[ "$CHANNEL" == "preview" && "$VERSION_WITHOUT_BUILD" != *-* ]]; then
  echo "Preview releases require a prerelease semver such as 0.2.0-beta.1" >&2
  exit 2
fi
if [[ "$CHANNEL" == "stable" && "$VERSION_WITHOUT_BUILD" == *-* ]]; then
  echo "Stable releases cannot use a prerelease semver; pass --preview instead" >&2
  exit 2
fi

# Load local signing configuration without printing it. Explicit environment
# values win over .env values.
if [[ -f "$ROOT/.env" ]]; then
  existing_key="${TAURI_SIGNING_PRIVATE_KEY+x}"
  existing_password="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD+x}"
  existing_identity="${APPLE_SIGNING_IDENTITY+x}"
  saved_key="${TAURI_SIGNING_PRIVATE_KEY:-}"
  saved_password="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}"
  saved_identity="${APPLE_SIGNING_IDENTITY:-}"
  set -a
  # shellcheck disable=SC1091
  source "$ROOT/.env"
  set +a
  [[ -n "$existing_key" ]] && TAURI_SIGNING_PRIVATE_KEY="$saved_key"
  [[ -n "$existing_password" ]] && TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$saved_password"
  [[ -n "$existing_identity" ]] && APPLE_SIGNING_IDENTITY="$saved_identity"
fi

fail() {
  echo "Release preflight failed: $*" >&2
  exit 1
}

# Every potentially failing prerequisite is checked before any version file is
# changed. A failed build later also restores the original version metadata.
[[ "$(uname -s)" == "Darwin" ]] || fail "macOS is required"
for command in git node pnpm python3 cargo rustc; do
  command -v "$command" >/dev/null || fail "missing required command: $command"
done
for command in /usr/bin/codesign /usr/bin/hdiutil /usr/bin/lipo /usr/bin/plutil /usr/bin/security /usr/bin/tar; do
  [[ -x "$command" ]] || fail "missing required command: $command"
done
if [[ "$NOTARIZED" == "1" ]]; then
  [[ -x /usr/sbin/spctl && -x /usr/bin/xcrun ]] || fail "Gatekeeper and Xcode tools are required for a notarized release"
fi
[[ -z "$(git status --porcelain --untracked-files=all)" ]] || fail "working tree must be clean"
[[ "$(git branch --show-current)" == "main" ]] || fail "releases must be cut from main"
[[ -n "$NOTES" ]] || fail "release notes must not be empty"
pnpm install --frozen-lockfile || fail "frozen frontend dependency install failed"

PKG_VER="$(node -p "require('./package.json').version")"
TAURI_VER="$(node -p "require('./src-tauri/tauri.conf.json').version")"
if ! CARGO_METADATA="$(cargo metadata --manifest-path src-tauri/Cargo.toml --locked --no-deps --format-version 1)"; then
  fail "Cargo.lock is missing or out of date"
fi
CARGO_VER="$(node -e '
  const metadata = JSON.parse(require("fs").readFileSync(0, "utf8"));
  const root = metadata.packages.find((pkg) => pkg.manifest_path === metadata.workspace_root + "/Cargo.toml");
  if (!root) process.exit(1);
  process.stdout.write(root.version);
' <<<"$CARGO_METADATA")" || fail "could not read the Cargo package version"
[[ "$PKG_VER" == "$TAURI_VER" && "$PKG_VER" == "$CARGO_VER" ]] || \
  fail "current versions disagree (package=$PKG_VER tauri=$TAURI_VER cargo=$CARGO_VER)"
[[ "$VERSION" != "$PKG_VER" ]] || fail "version is already $VERSION"
CURRENT_VERSION="$PKG_VER" NEXT_VERSION="$VERSION" node - <<'NODE' || fail "version must be greater than $PKG_VER"
const parse = (value) => {
  const [, major, minor, patch, prerelease] = value.match(/^(\d+)\.(\d+)\.(\d+)(?:-([^+]+))?(?:\+.*)?$/);
  return { core: [major, minor, patch].map(Number), pre: prerelease?.split(".") };
};
const compare = (a, b) => {
  for (let i = 0; i < 3; i++) if (a.core[i] !== b.core[i]) return Math.sign(a.core[i] - b.core[i]);
  if (!a.pre && !b.pre) return 0;
  if (!a.pre) return 1;
  if (!b.pre) return -1;
  for (let i = 0; i < Math.max(a.pre.length, b.pre.length); i++) {
    if (a.pre[i] === undefined) return -1;
    if (b.pre[i] === undefined) return 1;
    if (a.pre[i] === b.pre[i]) continue;
    const an = /^\d+$/.test(a.pre[i]);
    const bn = /^\d+$/.test(b.pre[i]);
    if (an && bn) return Math.sign(Number(a.pre[i]) - Number(b.pre[i]));
    if (an !== bn) return an ? -1 : 1;
    return a.pre[i] < b.pre[i] ? -1 : 1;
  }
  return 0;
};
process.exit(compare(parse(process.env.NEXT_VERSION), parse(process.env.CURRENT_VERSION)) > 0 ? 0 : 1);
NODE
git rev-parse -q --verify "refs/tags/v$VERSION" >/dev/null && fail "tag v$VERSION already exists"

[[ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ]] || fail "TAURI_SIGNING_PRIVATE_KEY is not set"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}"
if [[ "$NOTARIZED" == "1" ]]; then
  [[ -n "${APPLE_SIGNING_IDENTITY:-}" ]] || fail "APPLE_SIGNING_IDENTITY is not set"
  /usr/bin/security find-identity -v -p codesigning | grep -Fq "\"$APPLE_SIGNING_IDENTITY\"" || \
    fail "APPLE_SIGNING_IDENTITY is not available in the keychain"
  if ! { [[ -n "${APPLE_ID:-}" && -n "${APPLE_PASSWORD:-}" && -n "${APPLE_TEAM_ID:-}" ]] || \
         [[ -n "${APPLE_API_KEY:-}" && -n "${APPLE_API_ISSUER:-}" && -n "${APPLE_API_KEY_PATH:-}" ]]; }; then
    fail "Apple notarization credentials are not configured"
  fi
else
  export APPLE_SIGNING_IDENTITY="-"
  echo "! Community release: updater-signed and ad-hoc code signed, but not Apple-notarized." >&2
fi
if [[ "$PUBLISH" == "1" ]]; then
  command -v gh >/dev/null || fail "gh is required with --publish"
  gh auth status >/dev/null 2>&1 || fail "gh is not authenticated"
  if [[ "$CHANNEL" == "stable" ]]; then
    gh api "repos/nodelike/sikemux/git/ref/tags/v$VERSION" >/dev/null 2>&1 && fail "remote tag v$VERSION already exists"
    gh release view "v$VERSION" >/dev/null 2>&1 && fail "GitHub release v$VERSION already exists"
  fi
fi

if [[ "${RELEASE_PREFLIGHT_ONLY:-0}" == "1" ]]; then
  if [[ "$NOTARIZED" == "1" ]]; then mode="notarized"; else mode="community"; fi
  echo "✓ Release preflight passed ($mode mode)"
  exit 0
fi

echo "✓ Preflight passed; bumping version to $VERSION"
BACKUP="$(mktemp -d)"
FILES=(package.json src-tauri/tauri.conf.json src-tauri/Cargo.toml src-tauri/Cargo.lock latest.json)
for file in "${FILES[@]}"; do
  [[ -e "$file" ]] && { mkdir -p "$BACKUP/$(dirname "$file")"; cp "$file" "$BACKUP/$file"; }
done
SUCCESS=0
EXTRACTED=""
DMG_MOUNT=""
DMG_ATTACHED=0
restore_on_failure() {
  status=$?
  trap - EXIT INT TERM
  if [[ "$DMG_ATTACHED" == "1" ]]; then
    /usr/bin/hdiutil detach "$DMG_MOUNT" -quiet >/dev/null 2>&1 || true
  fi
  [[ -n "$EXTRACTED" ]] && rm -rf "$EXTRACTED"
  [[ -n "$DMG_MOUNT" ]] && rm -rf "$DMG_MOUNT"
  if [[ "$SUCCESS" != "1" ]]; then
    echo "Release failed; restoring version metadata." >&2
    for file in "${FILES[@]}"; do
      if [[ -e "$BACKUP/$file" ]]; then cp "$BACKUP/$file" "$file"; else rm -f "$file"; fi
    done
  fi
  rm -rf "$BACKUP"
  exit "$status"
}
trap restore_on_failure EXIT INT TERM

VERSION="$VERSION" python3 - <<'PY'
import json, os, pathlib, re
version = os.environ["VERSION"]
for name in ("package.json", "src-tauri/tauri.conf.json"):
    path = pathlib.Path(name)
    data = json.loads(path.read_text())
    data["version"] = version
    path.write_text(json.dumps(data, indent=2) + "\n")
path = pathlib.Path("src-tauri/Cargo.toml")
text = path.read_text()
updated, count = re.subn(r'(?m)^(version = ")[^"]+("\s*)$', rf'\g<1>{version}\2', text, count=1)
if count != 1:
    raise SystemExit("could not update Cargo.toml package version")
path.write_text(updated)
PY

BUNDLE="$ROOT/src-tauri/target/release/bundle"
APP_NAME="$(node -p "require('./src-tauri/tauri.conf.json').productName")"
TAR="$BUNDLE/macos/${APP_NAME}.app.tar.gz"
SIG="$TAR.sig"
APP="$BUNDLE/macos/${APP_NAME}.app"
# Remove stale outputs before the one release build. Their absence afterwards
# is stronger evidence of freshness than directory mtimes, which do not change
# when a bundler updates files in place.
rm -rf "$APP" "$TAR" "$SIG" "$BUNDLE/dmg"
echo "→ Building updater-signed v$VERSION"
REQUIRE_VALID_SIGNATURE=1 REQUIRE_SIGNED_APP="$NOTARIZED" "$ROOT/scripts/build-mac.sh"

[[ -d "$APP" ]] || fail "app bundle was not produced"
if [[ "$NOTARIZED" == "1" ]]; then
  /usr/sbin/spctl --assess --type execute --verbose=4 "$APP" || fail "app failed Gatekeeper assessment"
  /usr/bin/xcrun stapler validate "$APP" || fail "app has no valid notarization ticket"
fi
[[ -s "$TAR" ]] || fail "updater archive was not produced"
[[ -s "$SIG" ]] || fail "updater signature was not produced"
shopt -s nullglob
DMG_CANDIDATES=("$BUNDLE"/dmg/*.dmg)
shopt -u nullglob
[[ ${#DMG_CANDIDATES[@]} -eq 1 ]] || fail "expected exactly one DMG, found ${#DMG_CANDIDATES[@]}"
DMG="${DMG_CANDIDATES[0]}"
[[ -s "$DMG" ]] || fail "DMG is empty"
/usr/bin/hdiutil verify "$DMG" >/dev/null || fail "DMG verification failed"
node "$ROOT/scripts/verify-updater-signature.mjs" "$TAR" "$SIG"

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
APP_CDHASH="$(/usr/bin/codesign -dv --verbose=4 "$APP" 2>&1 | /usr/bin/sed -n 's/^CDHash=//p')"
ARCHIVE_CDHASH="$(/usr/bin/codesign -dv --verbose=4 "$ARCHIVE_APP" 2>&1 | /usr/bin/sed -n 's/^CDHash=//p')"
[[ -n "$APP_CDHASH" && "$ARCHIVE_CDHASH" == "$APP_CDHASH" ]] || fail "updater archive does not contain the exact signed app"

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
DMG_CDHASH="$(/usr/bin/codesign -dv --verbose=4 "$DMG_APP" 2>&1 | /usr/bin/sed -n 's/^CDHash=//p')"
[[ "$DMG_CDHASH" == "$APP_CDHASH" ]] || fail "DMG does not contain the exact signed app"
DMG_PLIST="$DMG_APP/Contents/Info.plist"
[[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$DMG_PLIST")" == "$VERSION" ]] || fail "DMG app version is wrong"
[[ "$(/usr/bin/lipo -archs "$DMG_APP/Contents/MacOS/$EXECUTABLE_NAME")" == "$ARCHS" ]] || fail "DMG app architectures differ from updater app"
/usr/bin/hdiutil detach "$DMG_MOUNT" -quiet
DMG_ATTACHED=0
rm -rf "$DMG_MOUNT" "$EXTRACTED"
DMG_MOUNT=""
EXTRACTED=""

PLATFORMS=()
[[ " $ARCHS " == *" arm64 "* ]] && PLATFORMS+=(darwin-aarch64)
[[ " $ARCHS " == *" x86_64 "* ]] && PLATFORMS+=(darwin-x86_64)
[[ ${#PLATFORMS[@]} -gt 0 ]] || fail "unsupported updater architecture set: $ARCHS"
for arch in $ARCHS; do
  [[ "$arch" == "arm64" || "$arch" == "x86_64" ]] || fail "unsupported executable architecture: $arch"
done

PUB_DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
RELEASE_TAG="v$VERSION"
[[ "$CHANNEL" == "preview" ]] && RELEASE_TAG=preview
TAR_URL="https://github.com/nodelike/sikemux/releases/download/$RELEASE_TAG/${APP_NAME}.app.tar.gz"
PLATFORM_LIST="${PLATFORMS[*]}" VERSION="$VERSION" NOTES="$NOTES" PUB_DATE="$PUB_DATE" SIG="$SIG" TAR_URL="$TAR_URL" python3 - <<'PY'
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
pathlib.Path("latest.json").write_text(json.dumps(manifest, indent=2) + "\n")
PY
python3 -m json.tool latest.json >/dev/null

STABLE_GH_CMD=(gh release create "v$VERSION" --title "v$VERSION" --notes "$NOTES" "$DMG" "$TAR" "$SIG" latest.json)
if [[ "$PUBLISH" == "1" && "$CHANNEL" == "stable" ]]; then
  echo "→ Publishing stable v$VERSION"
  "${STABLE_GH_CMD[@]}"
  echo "✓ Released stable v$VERSION"
elif [[ "$PUBLISH" == "1" ]]; then
  echo "→ Publishing preview v$VERSION"
  if gh release view preview >/dev/null 2>&1; then
    gh release upload preview "$DMG" "$TAR" "$SIG" latest.json --clobber
    gh release edit preview --title "v$VERSION preview" --notes "$NOTES" --prerelease
  else
    gh release create preview --target "$(git rev-parse HEAD)" --title "v$VERSION preview" --notes "$NOTES" --prerelease "$DMG" "$TAR" "$SIG" latest.json
  fi
  echo "✓ Released preview v$VERSION"
else
  echo "✓ Verified $CHANNEL release v$VERSION ($ARCHS)"
  echo "  $DMG"
  echo "  $TAR"
  echo "  $SIG"
  echo "  $ROOT/latest.json"
  echo "To publish:"
  if [[ "$CHANNEL" == "stable" ]]; then
    printf '  '; printf '%q ' "${STABLE_GH_CMD[@]}"; echo
  else
    echo "  rerun with --preview --publish (updates the moving preview release)"
  fi
fi

SUCCESS=1
