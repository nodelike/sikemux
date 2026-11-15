#!/usr/bin/env bash
# Works out, from GitHub and Sentry, whether a stable candidate is ready to ship, and writes the
# evidence on the run's page. Sets ready=true when it is, leaves it unset while the candidate is
# still soaking, and exits 1 when something is wrong with it, so it never ships.
#   SENTRY_READ_TOKEN=… scripts/ship-gate.sh <version> [minutes on staff] [Macs]
set -euo pipefail

cd "$(dirname "$0")/.."
VERSION="${1:?usage: $0 <version> [minutes on staff] [Macs]}"
MINUTES="${2:-30}"
MACS="${3:-2}"
SUMMARY="${GITHUB_STEP_SUMMARY:-/dev/stdout}"
OUTPUT="${GITHUB_OUTPUT:-/dev/null}"
[[ -n "${SENTRY_READ_TOKEN:-}" ]] || { echo "SENTRY_READ_TOKEN is not set" >&2; exit 1; }
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "v$VERSION is not a stable version" >&2; exit 1; }

problems=()
waiting=()
release="$(gh release view "v$VERSION" --json isPrerelease,name,publishedAt)"
[[ "$(jq -r .isPrerelease <<<"$release")" == true ]] || problems+=("v$VERSION is not a waiting candidate: it has shipped already")
[[ "$(jq -r .name <<<"$release")" != *"(pulled)" ]] || problems+=("v$VERSION was pulled")
staff="$(gh release download staff -p latest.json -O - 2>/dev/null | jq -r .version || true)"
[[ "$staff" == "$VERSION" ]] || problems+=("the staff feed serves ${staff:+v$staff}${staff:-nothing}, not the candidate")
published="$(jq -r .publishedAt <<<"$release")"
minutes=$((($(date +%s) - $(date -d "$published" +%s)) / 60))
((minutes >= MINUTES)) || waiting+=("the candidate has been on staff for $minutes min, under $MINUTES")

run="$(gh run list --workflow release.yml --event push --branch "v$VERSION" --limit 1 --json databaseId,conclusion,url --jq '.[0]')"
[[ "$(jq -r .conclusion <<<"$run")" == success ]] || problems+=("its Release run did not pass")
echo "release_run=$(jq -r .databaseId <<<"$run")" >>"$OUTPUT"

# Counts events in Sentry: the version's own crashes, and the report each update to it sends when
# it ends, from whichever build saw it end.
count() {
  curl -fsS -G -H "Authorization: Bearer $SENTRY_READ_TOKEN" \
    "https://de.sentry.io/api/0/organizations/nodelike/events/" \
    --data-urlencode "field=count()" --data-urlencode "query=$1" \
    --data-urlencode "project=-1" --data-urlencode "statsPeriod=14d" |
    jq -r '.data[0]["count()"] // 0'
}
if ! crashes="$(count "release:sikemux@$VERSION !logger:update")"; then
  echo "Sentry did not answer; SENTRY_READ_TOKEN needs the event:read and org:read scopes" >&2
  exit 1
fi
failed="$(count "logger:update tags[update.to]:$VERSION tags[update.result]:failed")"
rolled_back="$(count "logger:update tags[update.to]:$VERSION tags[update.result]:\"rolled back\"")"
updated="$(count "logger:update tags[update.to]:$VERSION tags[update.result]:done")"
macs="$(count "logger:update tags[update.to]:$VERSION tags[update.result]:done tags[update.platform]:darwin*")"
((crashes == 0)) || problems+=("$crashes crash reports from v$VERSION")
((failed == 0)) || problems+=("$failed updates to it failed")
((rolled_back == 0)) || problems+=("$rolled_back machines went back from it")
((macs >= MACS)) || waiting+=("$macs Macs updated to it, under $MACS")

headline="Ship v$VERSION · $minutes min on staff · $crashes crashes · $failed failed updates"
{
  echo "## $headline"
  echo
  echo "| | |"
  echo "|-|-|"
  echo "| Soak | $minutes min on the staff feed, $updated machines updated to it, $macs of them Macs |"
  echo "| Updates | $updated done, $failed failed, $rolled_back went back |"
  echo "| Crashes | $crashes from v$VERSION |"
  echo "| Rehearsal | $(jq -r .url <<<"$run"): the last stable updated itself to these files |"
  echo
  echo "### What's New"
  echo
  gh release view "v$VERSION" --json body --jq .body
} >>"$SUMMARY"

if ((${#problems[@]})); then
  printf 'Not ready: %s\n' "${problems[@]}" | tee -a "$SUMMARY" >&2
  exit 1
fi
if ((${#waiting[@]})); then
  printf 'Still soaking: %s\n' "${waiting[@]}" | tee -a "$SUMMARY"
  exit 0
fi
echo "ready=true" >>"$OUTPUT"
echo "v$VERSION is ready to ship"
