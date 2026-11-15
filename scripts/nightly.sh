#!/usr/bin/env bash
# Steers the nightlies the Release workflow builds from every push to main, as docs/releasing.md describes.
#   nightly.sh why                 what the staff and nightly feeds serve, and whether builds are held
#   nightly.sh hold [REASON] | unhold
#   nightly.sh pull <version>      keeps a published build from nightly and moves the feeds off it
#   nightly.sh promote <version>   moves the nightly feed to a build that soaked on staff (the Nightly soak workflow runs it)
set -euo pipefail

cd "$(dirname "$0")/.."
REPO=nodelike/sikemux
export GH_REPO="$REPO"

feed_version() {
  local manifest
  manifest="$(gh release download "$1" -p latest.json -O - 2>/dev/null)" || return 0
  node -pe 'JSON.parse(process.argv[1]).version' "$manifest"
}

pulled() {
  [[ "$(gh release view "v$1" --json name --jq .name)" == *"(pulled)" ]]
}

newest_first() {
  sort -t. -k1,1nr -k2,2nr -k3,3nr -k4,4nr <<<"${1//-nightly/}" | sed -E 's/^([0-9]+\.[0-9]+\.[0-9]+)\./\1-nightly./'
}

# Whether nightly $1 came after $2, comparing the release and then the nightly number.
later() {
  [[ "$1" != "$2" && "$(newest_first "$1"$'\n'"$2" | head -1)" == "$1" ]]
}

why() {
  local hold
  hold="$(gh variable get NIGHTLY_HOLD 2>/dev/null || true)"
  if [[ -n "$hold" ]]; then echo "held: $hold"; else echo "building every push to main once CI starts on it"; fi
  echo "staff feed: v$(feed_version staff)"
  echo "nightly feed: v$(feed_version nightly)"
  gh run list --workflow release.yml --limit 3 --json displayTitle,status,conclusion \
    --jq '.[] | "release: \(.displayTitle), \(.status) \(.conclusion // "")"'
}

promote() {
  local version="$1" current
  if pulled "$version"; then echo "v$version was pulled; the nightly feed stays where it is"; return; fi
  current="$(feed_version nightly)"
  if [[ -n "$current" ]] && ! later "$version" "$current"; then
    echo "the nightly feed already serves v$current"
    return
  fi
  scripts/move-feed.sh nightly "$version"
}

pull() {
  local version="$1" fallback feed
  gh release edit "v$version" --title "v$version (pulled)"
  fallback="$(newest_first "$(gh release list --limit 100 --json tagName,name,isPrerelease \
    --jq '.[] | select(.isPrerelease and (.tagName | test("^v.*-nightly\\.")) and (.name | endswith("(pulled)") | not)) | .tagName[1:]')" |
    while read -r candidate; do if later "$version" "$candidate"; then echo "$candidate"; break; fi; done)"
  for feed in staff nightly; do
    [[ "$(feed_version "$feed")" == "$version" ]] || continue
    if [[ -n "$fallback" ]]; then scripts/move-feed.sh "$feed" "$fallback"; else echo "no earlier build for the $feed feed to go back to" >&2; fi
  done
  echo "v$version is pulled: the nightly feed never takes it, and machines that installed it keep it until the next build"
}

case "${1:-}" in
  why) why ;;
  hold) gh variable set NIGHTLY_HOLD --body "${2:-by hand} since $(date '+%F %R')" ;;
  unhold) gh variable delete NIGHTLY_HOLD ;;
  pull | promote) [[ "${2:-}" == *-nightly.* ]] || { echo "Usage: $0 $1 <nightly version>" >&2; exit 2; }; "$1" "$2" ;;
  *) echo "Usage: $0 why|hold [REASON]|unhold|pull <version>|promote <version>" >&2; exit 2 ;;
esac
