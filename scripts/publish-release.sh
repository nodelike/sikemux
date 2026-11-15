#!/usr/bin/env bash
# Publish a release that scripts/release.sh built and verified.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd -P)"
VERSION="${1:-}"
NOTES="${2:-}"
CHANNEL=stable
[[ "${3:-}" == "--nightly" ]] && CHANNEL=nightly

fail() {
  echo "Release publish failed: $*" >&2
  exit 1
}

[[ -n "$VERSION" && -n "$NOTES" ]] || { echo "Usage: $0 <version> <notes> [--nightly]" >&2; exit 2; }
[[ "${GITHUB_ACTIONS:-}" == "true" ]] || fail "releases publish from the Release workflow"

# shellcheck source=scripts/release-shape.sh
source "$ROOT/scripts/release-shape.sh"
BUNDLE="$ROOT/target/release/bundle"
APP_NAME="$(node scripts/app-metadata.mjs name)"
TAR="$BUNDLE/macos/${APP_NAME}.app.tar.gz"
SIG="$TAR.sig"
if [[ "$CHANNEL" == "stable" ]]; then
  MANIFEST="$ROOT/latest.json"
else
  MANIFEST="$BUNDLE/latest.json"
fi
shopt -s nullglob
DMGS=("$BUNDLE"/dmg/*.dmg)
DEBS=("$BUNDLE"/deb/*.deb)
RPMS=("$BUNDLE"/rpm/*.rpm)
APPIMAGES=("$BUNDLE"/appimage/*.AppImage)
TARBALLS=("$BUNDLE"/tar/*.tar.gz)
PACMAN_PACKAGES=("$BUNDLE"/arch/*.pkg.tar.zst)
HOSTS=("$ROOT"/target/host/sikemux-host-"$VERSION"-linux-*.tar.gz)
SYMBOLS=("$ROOT"/target/symbols/Sikemux_"$VERSION"_*.dSYM.zip)
shopt -u nullglob
[[ ${#DMGS[@]} -eq 1 ]] || fail "expected exactly one DMG, found ${#DMGS[@]}"
[[ ${#DEBS[@]} -eq 2 ]] || fail "expected the amd64 and arm64 debs, found ${#DEBS[@]}"
[[ ${#RPMS[@]} -eq 2 ]] || fail "expected the x86_64 and aarch64 rpms, found ${#RPMS[@]}"
[[ ${#APPIMAGES[@]} -eq 2 ]] || fail "expected the amd64 and arm64 AppImages, found ${#APPIMAGES[@]}"
[[ ${#TARBALLS[@]} -eq 2 ]] || fail "expected the amd64 and arm64 Linux tarballs, found ${#TARBALLS[@]}"
[[ ${#PACMAN_PACKAGES[@]} -eq 1 ]] || fail "expected exactly one Arch package, found ${#PACMAN_PACKAGES[@]}"
[[ ${#HOSTS[@]} -eq 2 ]] || fail "expected the x86_64 and aarch64 host cores, found ${#HOSTS[@]}"
[[ ${#SYMBOLS[@]} -eq 3 ]] || fail "expected the Mac app's, core's and notch helper's symbols, found ${#SYMBOLS[@]}"
HOST_SIGS=("${HOSTS[@]/%/.sig}")
DMG="${DMGS[0]}"
HELPERS=("${PUBLISHED_HELPER_FILES[@]/#/$ROOT/packaging/binaries/}")
APPIMAGE_SIGS=("${APPIMAGES[@]/%/.sig}")
PACMAN_PACKAGE="${PACMAN_PACKAGES[0]}"
INSTALLER="$ROOT/scripts/install.sh"
PACKAGES_INDEX="$ROOT/target/packages/packages-index-$VERSION.tar"
for file in "$PACKAGES_INDEX" "$DMG" "$TAR" "$SIG" "${HELPERS[@]}" "${DEBS[@]}" "${RPMS[@]}" "${APPIMAGES[@]}" "${HOSTS[@]}" "${HOST_SIGS[@]}" "$INSTALLER" "${APPIMAGE_SIGS[@]}" "${SYMBOLS[@]}" "${TARBALLS[@]}" "$PACMAN_PACKAGE" "$MANIFEST"; do
  [[ -s "$file" ]] || fail "$file is missing or empty"
done
[[ "$(node -p "require('$MANIFEST').version")" == "$VERSION" ]] || fail "$MANIFEST is not for v$VERSION"
for signed in "${HOSTS[@]}" "${APPIMAGES[@]}"; do
  node scripts/verify-updater-signature.mjs "$signed" "$signed.sig" || fail "$signed is not signed by the updater key"
done
# The Linux app updates itself only as an AppImage, so only the AppImages join the
# feed the macOS build wrote, beside each host core, which root installs on a host.
RELEASE_URL="https://github.com/nodelike/sikemux/releases/download/v$VERSION" \
  MANIFEST="$MANIFEST" python3 - "${APPIMAGES[@]}" "${HOSTS[@]}" <<'PY'
import json, os, pathlib, re, sys
path = pathlib.Path(os.environ["MANIFEST"])
manifest = json.loads(path.read_text())

def entry(file):
    file = pathlib.Path(file)
    return {
        "signature": pathlib.Path(f"{file}.sig").read_text().strip(),
        "url": f'{os.environ["RELEASE_URL"]}/{file.name}',
    }

for file in sys.argv[1:]:
    if file.endswith(".AppImage"):
        arch = {"amd64": "x86_64", "arm64": "aarch64"}[re.search(r"_([a-z0-9]+)\.AppImage$", file).group(1)]
        manifest["platforms"][f"linux-{arch}"] = entry(file)
    else:
        arch = re.search(r"-linux-([a-z0-9_]+)\.tar\.gz$", file).group(1)
        manifest["platforms"][f"host-linux-{arch}"] = entry(file)
path.write_text(json.dumps(manifest, indent=2) + "\n")
PY

RELEASE_CMD=(gh release create "v$VERSION" --verify-tag --title "v$VERSION" --notes "$NOTES" "$PACKAGES_INDEX" "$DMG" "$TAR" "$SIG" "${HELPERS[@]}" "${DEBS[@]}" "${RPMS[@]}" "${APPIMAGES[@]}" "${HOSTS[@]}" "${HOST_SIGS[@]}" "$INSTALLER" "${APPIMAGE_SIGS[@]}" "${SYMBOLS[@]}" "${TARBALLS[@]}" "$PACMAN_PACKAGE")
# The staff feed takes a nightly unless it serves a newer build, or a stable candidate that
# has not shipped yet: staff machines keep running the candidate the owner is asked about.
staff_feed_takes() {
  local serving
  serving="$(gh release download staff -p latest.json -O - 2>/dev/null | node -pe 'JSON.parse(require("fs").readFileSync(0)).version' 2>/dev/null)" || return 0
  if [[ "$serving" != *-* ]] &&
    [[ "$(gh release view "v$serving" --json isPrerelease,name --jq '.isPrerelease and (.name | endswith("(pulled)") | not)')" == true ]]; then
    echo "  the staff feed keeps the candidate v$serving until it ships or is pulled"
    return 1
  fi
  node scripts/newer-version.mjs "$1" "$serving" || { echo "  the staff feed already serves v$serving"; return 1; }
}

if [[ "$CHANNEL" == "stable" ]]; then
  echo "→ Publishing stable v$VERSION as a candidate for staff"
  "${RELEASE_CMD[@]}" "$MANIFEST" --prerelease
  scripts/move-feed.sh staff "$VERSION"
  echo "✓ v$VERSION is on the staff feed; the Ship workflow makes it stable once it has soaked"
else
  echo "→ Publishing nightly v$VERSION"
  # A push to main brings no tag, so Release makes it; the owner may still push one by hand.
  gh api "repos/{owner}/{repo}/git/ref/tags/v$VERSION" >/dev/null 2>&1 ||
    gh api -X POST "repos/{owner}/{repo}/git/refs" -f "ref=refs/tags/v$VERSION" -f "sha=$(git rev-parse HEAD)" >/dev/null
  "${RELEASE_CMD[@]}" "$MANIFEST" --prerelease
  if staff_feed_takes "$VERSION"; then scripts/move-feed.sh staff "$VERSION"; fi
  gh workflow run nightly-soak.yml -f "version=$VERSION"
  echo "✓ Released nightly v$VERSION; the nightly feed follows once it has soaked"
fi
