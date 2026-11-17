#!/usr/bin/env bash
# Points an update feed, the fixed `staff` or `nightly` release holding only latest.json, at a
# published build by copying that build's own latest.json. Nothing is rebuilt. The staff feed
# also serves stable candidates; the nightly feed only nightlies.
#   move-feed.sh <staff|nightly> <version>
set -euo pipefail

cd "$(dirname "$0")/.."
FEED="${1:-}"
VERSION="${2:-}"
[[ "$FEED" == staff || ( "$FEED" == nightly && "$VERSION" == *-nightly.* ) ]] && [[ -n "$VERSION" ]] || {
  echo "Usage: $0 <staff|nightly> <version>" >&2
  exit 2
}

TITLE="$( [[ "$FEED" == staff ]] && echo Staff || echo Nightly ) feed (v$VERSION)"
dir="$(mktemp -d)"
trap 'rm -rf "$dir"' EXIT
gh release download "v$VERSION" -p latest.json -D "$dir"
MANIFEST="$dir/latest.json"
[[ "$(node -p "require('$MANIFEST').version")" == "$VERSION" ]] || { echo "v$VERSION carries another version's latest.json" >&2; exit 1; }

NOTES="Update feed for the $FEED channel.

The installable build for this feed is [v$VERSION](https://github.com/nodelike/sikemux/releases/tag/v$VERSION).

This release carries only \`latest.json\`. Its \`$FEED\` tag is a fixed URL anchor that shipped clients resolve against, not a source revision — do not attach builds here."
if gh release view "$FEED" >/dev/null 2>&1; then
  gh release upload "$FEED" "$MANIFEST" --clobber
  gh release edit "$FEED" --title "$TITLE" --notes "$NOTES" --prerelease
else
  gh release create "$FEED" --target "$(gh api "repos/{owner}/{repo}/commits/v$VERSION" --jq .sha)" \
    --title "$TITLE" --notes "$NOTES" --prerelease "$MANIFEST"
fi
echo "✓ The $FEED feed now points at v$VERSION"
[[ "$FEED" == staff ]] || gh workflow run packages.yml -f suite=nightly
