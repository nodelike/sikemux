#!/usr/bin/env bash
# Build, sign and verify a macOS release, building and signing in separate steps
# so the build's dependencies never see the signing material. The Release
# workflow runs each step on its own machine: --stage helpers, scripts/sign-helper.sh,
# --stage compile, --stage package, scripts/sign-mac.sh, then scripts/verify-release.sh. --publish
# marks a build the workflow publishes with scripts/publish-release.sh.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd -P)"
VERSION="${1:-}"
NOTES="${2:-}"
PUBLISH="${RELEASE_PUBLISH:-0}"
NOTARIZED="${RELEASE_NOTARIZED:-0}"
CHANNEL="${RELEASE_CHANNEL:-stable}"
STAGE=all
options=("${@:3}")
for ((i = 0; i < ${#options[@]}; i++)); do
  case "${options[$i]}" in
    --publish) PUBLISH=1 ;;
    --nightly) CHANNEL=nightly ;;
    --stage) STAGE="${options[$((i + 1))]:-}"; i=$((i + 1)) ;;
    *) echo "Unknown option: ${options[$i]}" >&2; exit 2 ;;
  esac
done

if [[ -z "$VERSION" ]]; then
  echo "Usage: $0 <version> <notes> [--nightly] [--publish] [--stage helpers|compile|package]" >&2
  exit 2
fi
if [[ "$STAGE" != "all" && "$STAGE" != "helpers" && "$STAGE" != "compile" && "$STAGE" != "package" ]]; then
  echo "--stage must be helpers, compile or package" >&2
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
if [[ "$CHANNEL" != "stable" && "$CHANNEL" != "nightly" ]]; then
  echo "RELEASE_CHANNEL must be stable or nightly" >&2
  exit 2
fi
VERSION_WITHOUT_BUILD="${VERSION%%+*}"
if [[ "$CHANNEL" == "nightly" && "$VERSION_WITHOUT_BUILD" != *-* ]]; then
  echo "Nightly releases require a prerelease semver such as 0.4.0-nightly.1" >&2
  exit 2
fi
if [[ "$CHANNEL" == "stable" && "$VERSION_WITHOUT_BUILD" == *-* ]]; then
  echo "Stable releases cannot use a prerelease semver; pass --nightly instead" >&2
  exit 2
fi

SIGNING_VARS=(TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD APPLE_SIGNING_IDENTITY
  APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID APPLE_API_KEY APPLE_API_ISSUER APPLE_API_KEY_PATH
  APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD)
# Load local signing configuration without printing it. Explicit environment
# values win over .env values.
for name in "${SIGNING_VARS[@]}"; do
  if [[ -n "${!name+x}" ]]; then printf -v "EXPLICIT_$name" '%s' "${!name}"; fi
done
if [[ -f "$ROOT/.env" ]]; then
  set -a
  # shellcheck disable=SC1091
  source "$ROOT/.env"
  set +a
fi
# The signing material leaves the environment here, so dependency installs and
# build tools never see it; only the signing steps get it back.
for name in "${SIGNING_VARS[@]}"; do
  explicit="EXPLICIT_$name"
  if [[ -n "${!explicit+x}" ]]; then
    printf -v "SIGNING_$name" '%s' "${!explicit}"
  elif [[ -n "${!name+x}" ]]; then
    printf -v "SIGNING_$name" '%s' "${!name}"
  fi
  unset "$explicit" "$name"
done
signing() (
  for name in "${SIGNING_VARS[@]}"; do
    value="SIGNING_$name"
    if [[ -n "${!value+x}" ]]; then export "$name=${!value}"; fi
  done
  "$@"
)
signing_value() {
  local value="SIGNING_$1"
  printf '%s' "${!value:-}"
}

fail() {
  echo "Release preflight failed: $*" >&2
  exit 1
}

# Every potentially failing prerequisite is checked before any version file is
# changed. A failed build later also restores the original version metadata.
[[ "$(uname -s)" == "Darwin" ]] || fail "macOS is required"
for command in git node python3 cargo rustc; do
  command -v "$command" >/dev/null || fail "missing required command: $command"
done
[[ "$STAGE" == "helpers" ]] || command -v pnpm >/dev/null || fail "missing required command: pnpm"
for command in /usr/bin/codesign /usr/bin/hdiutil /usr/bin/lipo /usr/bin/plutil /usr/bin/security /usr/bin/tar; do
  [[ -x "$command" ]] || fail "missing required command: $command"
done
if [[ "$NOTARIZED" == "1" ]]; then
  [[ -x /usr/sbin/spctl && -x /usr/bin/xcrun ]] || fail "Gatekeeper and Xcode tools are required for a notarized release"
fi
[[ -z "$(git status --porcelain --untracked-files=all)" ]] || fail "working tree must be clean"
# Stable ships from a release line so a hotfix can go out while main runs ahead.
# A tag build checks out a detached commit, so the check is which branches hold it.
HEAD_SHA="$(git rev-parse HEAD)"
if [[ "$CHANNEL" == "stable" ]]; then
  [[ -n "$(git for-each-ref --contains HEAD refs/heads/release/ refs/remotes/origin/release/)" ]] || \
    fail "stable releases must be cut from a release/<line> branch; $HEAD_SHA is on none"
else
  [[ -n "$(git for-each-ref --contains HEAD refs/heads/main refs/remotes/origin/main)" ]] || \
    fail "nightly releases must be cut from main; $HEAD_SHA is not on it"
fi
[[ -n "$NOTES" ]] || fail "release notes must not be empty"
if [[ "$STAGE" != "helpers" ]]; then
  pnpm install --frozen-lockfile || fail "frozen frontend dependency install failed"
fi

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
CURRENT_VERSION="$PKG_VER" NEXT_VERSION="$VERSION" node - <<'NODE' || fail "version must not be less than $PKG_VER"
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
process.exit(compare(parse(process.env.NEXT_VERSION), parse(process.env.CURRENT_VERSION)) >= 0 ? 0 : 1);
NODE
# A pushed version tag is what starts a CI release, so only a tag elsewhere is a conflict.
if TAG_SHA="$(git rev-parse -q --verify "refs/tags/v$VERSION^{commit}")"; then
  [[ "$TAG_SHA" == "$HEAD_SHA" ]] || fail "tag v$VERSION already exists on $TAG_SHA"
fi

if [[ "$STAGE" == "all" ]]; then
  [[ -n "$(signing_value TAURI_SIGNING_PRIVATE_KEY)" ]] || fail "TAURI_SIGNING_PRIVATE_KEY is not set"
  if [[ "$NOTARIZED" == "1" ]]; then
    identity="$(signing_value APPLE_SIGNING_IDENTITY)"
    [[ -n "$identity" ]] || fail "APPLE_SIGNING_IDENTITY is not set"
    /usr/bin/security find-identity -v -p codesigning | grep -Fq "\"$identity\"" || \
      fail "APPLE_SIGNING_IDENTITY is not available in the keychain"
    if ! { [[ -n "$(signing_value APPLE_ID)" && -n "$(signing_value APPLE_PASSWORD)" && -n "$(signing_value APPLE_TEAM_ID)" ]] || \
           [[ -n "$(signing_value APPLE_API_KEY)" && -n "$(signing_value APPLE_API_ISSUER)" && -n "$(signing_value APPLE_API_KEY_PATH)" ]]; }; then
      fail "Apple notarization credentials are not configured"
    fi
  else
    # shellcheck disable=SC2034 # signing() hands it to the signing steps
    SIGNING_APPLE_SIGNING_IDENTITY=-
    echo "! Community release: updater-signed and ad-hoc code signed, but not Apple-notarized." >&2
  fi
fi
# The Release workflow checks the tag and the GitHub release before it builds.
if [[ "$PUBLISH" == "1" ]]; then
  [[ "${GITHUB_ACTIONS:-}" == "true" ]] || fail "releases publish from the Release workflow; run this locally without --publish to preview"
  export SIKEMUX_USAGE_REPORTING=app
fi

if [[ "${RELEASE_PREFLIGHT_ONLY:-0}" == "1" ]]; then
  if [[ "$NOTARIZED" == "1" ]]; then mode="notarized"; else mode="community"; fi
  echo "✓ Release preflight passed ($mode mode)"
  exit 0
fi

if [[ "$VERSION" == "$PKG_VER" ]]; then
  echo "✓ Preflight passed; using prepared version $VERSION"
else
  echo "✓ Preflight passed; bumping version to $VERSION"
fi
BACKUP="$(mktemp -d)"
FILES=(package.json src-tauri/tauri.conf.json src-tauri/Cargo.toml src-tauri/Cargo.lock latest.json)
for file in "${FILES[@]}"; do
  if [[ -e "$file" ]]; then mkdir -p "$BACKUP/$(dirname "$file")"; cp "$file" "$BACKUP/$file"; fi
done
SUCCESS=0
EXPECTED=""
restore_on_failure() {
  status=$?
  trap - EXIT INT TERM
  if [[ "$SUCCESS" != "1" ]]; then
    echo "Release failed; restoring version metadata." >&2
    for file in "${FILES[@]}"; do
      # Only undo what this script wrote. Anything edited meanwhile is someone
      # else's and would be silently reverted.
      if [[ -n "$EXPECTED" ]] && ! cmp -s "$EXPECTED/$file" "$file"; then
        echo "! $file changed during the release; left as it is." >&2
        continue
      fi
      if [[ -e "$BACKUP/$file" ]]; then cp "$BACKUP/$file" "$file"; else rm -f "$file"; fi
    done
  fi
  rm -rf "$BACKUP"
  [[ -n "$EXPECTED" ]] && rm -rf "$EXPECTED"
  exit "$status"
}
snapshot_expected() {
  [[ -n "$EXPECTED" ]] && rm -rf "$EXPECTED"
  EXPECTED="$(mktemp -d)"
  for file in "${FILES[@]}"; do
    if [[ -e "$file" ]]; then mkdir -p "$EXPECTED/$(dirname "$file")"; cp "$file" "$EXPECTED/$file"; fi
  done
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
# Cargo.lock records the workspace version and the sidecar builds with --locked,
# so the bump has to reach the lock file or the build fails partway through.
if [[ "$VERSION" != "$PKG_VER" ]]; then
  cargo metadata --manifest-path src-tauri/Cargo.toml --offline --format-version 1 >/dev/null \
    || fail "could not refresh Cargo.lock for $VERSION"
fi
snapshot_expected

BUNDLE="$ROOT/src-tauri/target/release/bundle"
APP_NAME="$(node -p "require('./src-tauri/tauri.conf.json').productName")"
APP="$BUNDLE/macos/${APP_NAME}.app"
HOST="$(rustc -vV | sed -n 's/^host: //p')"
VOICE="$ROOT/src-tauri/binaries/sikemux-voice-$HOST"
SIM="$ROOT/src-tauri/binaries/sikemux-sim-$HOST"
# Remove stale outputs before the one release build. Their absence afterwards
# is stronger evidence of freshness than directory mtimes, which do not change
# when a bundler updates files in place.
if [[ "$STAGE" == "all" || "$STAGE" == "helpers" ]]; then
  rm -f "$VOICE" "$SIM"
  echo "→ Building the voice and simulator helpers for v$VERSION"
  node "$ROOT/scripts/build-voice-helper.mjs"
  node "$ROOT/scripts/build-sim-helper.mjs"
fi
if [[ "$STAGE" == "all" ]]; then
  signing "$ROOT/scripts/sign-helper.sh" "$VOICE" "$SIM"
fi
if [[ "$STAGE" != "helpers" ]]; then
  [[ -x "$VOICE" && -x "$SIM" ]] || fail "the signed voice and simulator helpers are missing"
  rm -rf "$APP" "$BUNDLE/macos/${APP_NAME}.app.tar.gz" "$BUNDLE/macos/${APP_NAME}.app.tar.gz.sig" "$BUNDLE/dmg"
  echo "→ Building v$VERSION"
  step=all
  [[ "$STAGE" == "compile" || "$STAGE" == "package" ]] && step="$STAGE"
  BUILD_STEP="$step" REQUIRE_VALID_SIGNATURE=1 PREBUILT_HELPERS=1 "$ROOT/scripts/build-mac.sh"
  [[ "$STAGE" == "compile" || -d "$APP" ]] || fail "app bundle was not produced"
fi
if [[ "$STAGE" == "all" ]]; then
  signing "$ROOT/scripts/sign-mac.sh" "$VERSION"
  flags=()
  [[ "$CHANNEL" == "nightly" ]] && flags+=(--nightly)
  "$ROOT/scripts/verify-release.sh" "$VERSION" "$NOTES" ${flags[@]+"${flags[@]}"}
fi
snapshot_expected

[[ "$(git rev-parse HEAD)" == "$HEAD_SHA" ]] || fail "HEAD moved during the release; rebuild from a settled tree"
case "$STAGE" in
  helpers) echo "✓ Built the helpers for v$VERSION; sign them with scripts/sign-helper.sh before the app" ;;
  compile) echo "✓ Compiled v$VERSION; pack it with --stage package" ;;
  package) echo "✓ Built v$VERSION; sign it with scripts/sign-mac.sh" ;;
  all) echo "✓ Verified $CHANNEL release v$VERSION" ;;
esac
if [[ "$PUBLISH" == "1" ]]; then
  echo "  The Release workflow signs and verifies it, and publishes once the checks pass."
fi

SUCCESS=1
