#!/usr/bin/env bash
# Points a package repository suite, the fixed `packages-stable` or `packages-nightly`
# release apt reads, at the release its update feed
# names, by copying the signed index that release carries. Nothing is rebuilt or signed.
# Stable follows the latest stable release; nightly follows the nightly feed, or stable
# when that is newer, as the app does. Taking the target from the feeds lets a run that
# replaced another one waiting do that one's move too. Run it only from the Package
# repository workflow, which moves one suite at a time:
#   gh workflow run packages.yml -f suite=<stable|nightly>
#
# apt reads InRelease, then the index it names. Each move switches the index between
# Packages.xz and Packages.gz, so the previous InRelease keeps naming a file that is
# still there. InRelease is replaced by renaming a finished upload. For the one call in
# between, apt falls back to Release and Release.gpg, which still match the previous
# index, so no apt client ever sees two files that disagree.
set -euo pipefail

cd "$(dirname "$0")/.."
SUITE="${1:-}"
[[ "$SUITE" == stable || "$SUITE" == nightly ]] || {
  echo "Usage: $0 <stable|nightly>" >&2
  exit 2
}
TAG="packages-$SUITE"
# An apt that read the previous InRelease fetches its index within seconds.
OLD_INDEX_KEPT_FOR=60

# Orders 0.7.0-nightly.3 after 0.6.2 and before 0.7.0, as apt does.
order() {
  local base="${1%%-*}" build=999999 major minor patch
  [[ "$1" == *-nightly.* ]] && build="${1##*-nightly.}"
  IFS=. read -r major minor patch <<<"$base"
  printf '%06d%06d%06d%06d' "$major" "$minor" "$patch" "$build"
}

VERSION="$(gh release view --json tagName --jq .tagName)"
VERSION="${VERSION#v}"
if [[ "$SUITE" == nightly ]]; then
  nightly="$(gh release download nightly -p latest.json -O - | jq -r .version)"
  [[ "$(order "$nightly")" > "$(order "$VERSION")" ]] && VERSION="$nightly"
fi
if ! gh release view "v$VERSION" --json assets --jq '.assets[].name' | grep -qx "packages-index-$VERSION.tar"; then
  echo "✓ v$VERSION carries no package index, so the $SUITE packages stay as they are"
  exit 0
fi

dir="$(mktemp -d)"
trap 'rm -rf "$dir"' EXIT
gh release download "v$VERSION" -p "packages-index-$VERSION.tar" -D "$dir"
mkdir "$dir/index"
tar -xf "$dir/packages-index-$VERSION.tar" -C "$dir/index"

TITLE="Packages, $SUITE (v$VERSION)"
NOTES="The $SUITE suite of Sikemux's apt repository.

It serves [v$VERSION](https://github.com/nodelike/sikemux/releases/tag/v$VERSION). Its \`$TAG\` tag is a fixed address machines with the .deb read, not a source revision: do not attach builds here."

asset_id() {
  gh api "repos/{owner}/{repo}/releases/tags/$TAG" --jq ".assets[] | select(.name == \"$1\") | .id"
}

replace() {
  local name="$1" file="$2" old new
  cp "$file" "$dir/$name.next"
  gh release upload "$TAG" "$dir/$name.next" --clobber
  old="$(asset_id "$name")"
  new="$(asset_id "$name.next")"
  [[ -z "$old" ]] || gh api -X DELETE "repos/{owner}/{repo}/releases/assets/$old" >/dev/null
  gh api -X PATCH "repos/{owner}/{repo}/releases/assets/$new" -f name="$name" >/dev/null
}

if ! gh release view "$TAG" >/dev/null 2>&1; then
  apt="$dir/index/apt/$SUITE/xz"
  gh release create "$TAG" --target "$(gh api "repos/{owner}/{repo}/commits/v$VERSION" --jq .sha)" \
    --title "$TITLE" --notes "$NOTES" --prerelease --latest=false \
    "$apt/Packages.xz" "$apt/InRelease" "$apt/Release" "$apt/Release.gpg"
else
  if [[ "$(gh release view "$TAG" --json name --jq .name)" == "$TITLE" ]]; then
    echo "✓ The $SUITE packages already serve v$VERSION"
    exit 0
  fi
  if [[ -n "$(asset_id Packages.xz)" ]]; then format=gz old=xz; else format=xz old=gz; fi
  apt="$dir/index/apt/$SUITE/$format"
  gh release upload "$TAG" "$apt/Packages.$format" --clobber
  replace InRelease "$apt/InRelease"
  replace Release "$apt/Release"
  replace Release.gpg "$apt/Release.gpg"
  gh release edit "$TAG" --title "$TITLE" --notes "$NOTES" --prerelease --latest=false
  sleep "$OLD_INDEX_KEPT_FOR"
  stale="$(asset_id "Packages.$old")"
  [[ -z "$stale" ]] || gh api -X DELETE "repos/{owner}/{repo}/releases/assets/$stale" >/dev/null
fi
echo "✓ The $SUITE packages now serve v$VERSION"
