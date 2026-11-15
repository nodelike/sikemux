#!/usr/bin/env bash
# Ships the next nightly from main in two steps, as docs/releasing.md describes.
#   nightly.sh check              prints the next step once one can run, else the last answer
#   nightly.sh why                prints the next step, or why none can run now
#   nightly.sh tag [--dry-run] [--notes FILE]   runs the next step
#   nightly.sh hold [REASON] | unhold
# First a bump: once main has commits since the last nightly and that nightly is at least
# SIKEMUX_NIGHTLY_HOURS (12) old, the version bump and notes are committed on origin/main's tip
# from a temporary index, so no checkout moves. Then the tag: the newest commit since the bump
# that passed CI, with every other push workflow's newest verdict before it green, gets the tag
# that starts Release.
set -euo pipefail

cd "$(dirname "$0")/.."
REPO=nodelike/sikemux
HOURS="${SIKEMUX_NIGHTLY_HOURS:-12}"
WORKFLOWS=(ci.yml mobile-ci.yml server.yml docs.yml)
HOLD="${XDG_CONFIG_HOME:-$HOME/.config}/sikemux/release/nightly-hold"

fetch() {
  git fetch --quiet origin main
  git fetch --quiet --tags --force origin
}

last_tag() {
  git tag -l 'v*-nightly.*' --sort=-v:refname | head -1
}

version_at() {
  git show "$1:package.json" | node -pe 'JSON.parse(require("fs").readFileSync(0)).version'
}

tagged() {
  git rev-parse -q --verify "refs/tags/v$1" >/dev/null
}

pending_version() {
  local current
  current="$(version_at origin/main)"
  if [[ "$current" == *-nightly.* ]] && ! tagged "$current"; then echo "$current"; fi
}

next_version() {
  local current core
  current="$(version_at origin/main)"
  core="${current%%-*}"
  if tagged "$core"; then
    IFS=. read -r major minor patch <<<"$core"
    echo "$major.$minor.$((patch + 1))-nightly.1"
  elif [[ "$current" != *-nightly.* ]]; then
    echo "$core-nightly.1"
  else
    echo "$core-nightly.$((${current##*.} + 1))"
  fi
}

newest_green_since() {
  local bump="$1" revs runs
  revs="$(git rev-list --first-parent -n 400 origin/main | jq -Rsc 'split("\n") | map(select(length > 0))')"
  runs="$(for workflow in "${WORKFLOWS[@]}"; do
    gh run list -R "$REPO" --branch main --event push --workflow "$workflow" --limit 60 \
      --json headSha,status,conclusion --jq ".[] + {workflow: \"$workflow\"}"
  done | jq -sc .)"
  jq -nr --argjson revs "$revs" --argjson runs "$runs" --arg bump "$bump" '
    ($revs | to_entries | map({key: .value, value: .key}) | from_entries) as $at
    | [$runs[] | select($at[.headSha] != null and .conclusion != "cancelled" and .conclusion != "skipped")
        | .at = $at[.headSha]] as $verdicts
    | first(range(0; $at[$bump] + 1) as $i
        | select(any($verdicts[]; .workflow == "ci.yml" and .at == $i and .conclusion == "success"))
        | select(all($verdicts | map(select(.workflow != "ci.yml" and .at >= $i)) | group_by(.workflow)[];
            min_by(.at).conclusion == "success"))
        | $revs[$i]) // empty'
}

next_step() {
  if [[ -e "$HOLD" ]]; then echo "wait: held, $(cat "$HOLD")"; return; fi
  [[ "$(gh run list -R "$REPO" --workflow release.yml --limit 5 --json status \
    --jq '[.[] | select(.status != "completed")] | length')" == 0 ]] || { echo "wait: a Release run is going"; return; }
  local version bump green last age
  version="$(pending_version)"
  if [[ -n "$version" ]]; then
    bump="$(git log -1 --format=%H -S"\"version\": \"$version\"" origin/main -- package.json)"
    green="$(newest_green_since "$bump")"
    if [[ -n "$green" ]]; then echo "tag $version $green"; else echo "wait: nothing since the v$version bump has passed its checks yet"; fi
    return
  fi
  last="$(last_tag)"
  [[ -n "$(git rev-list -1 "$last..origin/main")" ]] || { echo "wait: main has nothing since $last"; return; }
  age=$(($(date +%s) - $(git log -1 --format=%ct "$last^{commit}")))
  ((age >= HOURS * 3600)) || { echo "wait: $last is only $((age / 3600))h old"; return; }
  echo "bump $(next_version)"
}

check() {
  local answer="${TMPDIR:-/tmp}/sikemux-nightly-last" step=""
  fetch 2>/dev/null && step="$(next_step)"
  if [[ "$step" == bump* || "$step" == tag* ]]; then
    echo "$step" | tee "$answer"
  else
    cat "$answer" 2>/dev/null || true
  fi
}

notes() {
  local version="$1" last="$2"
  printf '# Sikemux v%s\n\n' "$version"
  git show origin/main:RELEASE_NOTES.md | awk 'NR > 1 && NF { found = 1 } found && !NF { exit } found'
  printf '\n## New since %s\n\n' "${last#v}"
  git log --reverse --no-merges --format=%s "$last..origin/main" |
    grep -E '^(feat|fix|perf)(\([^)]*\))?!?: ' |
    sed -E 's/^[a-z]+(\(([^)]*)\))?!?: (.*)$/- \3/; s/^(- [^.]*\.)( .*)?$/\1/'
  printf "\nFor the complete patch history, compare [\`%s...v%s\`](https://github.com/%s/compare/%s...v%s).\n" \
    "$last" "$version" "$REPO" "$last" "$version"
}

bump() {
  local version="$1" dry="$2" notes_file="$3" index base from last commit
  index="$(mktemp)"
  base="$(git rev-parse origin/main)"
  from="$(version_at "$base")"
  last="$(last_tag)"
  put() { git update-index --add --cacheinfo "100644,$(git hash-object -w --stdin),$1"; }
  export GIT_INDEX_FILE="$index"
  git read-tree "$base"
  git show "$base:package.json" | sed "s/\"version\": \"$from\"/\"version\": \"$version\"/" | put package.json
  for file in Cargo.toml Cargo.lock; do
    git show "$base:$file" | sed "s/^version = \"$from\"$/version = \"$version\"/" | put "$file"
  done
  if [[ -n "$notes_file" ]]; then put RELEASE_NOTES.md <"$notes_file"; else notes "$version" "$last" | put RELEASE_NOTES.md; fi
  commit="$(git commit-tree "$(git write-tree)" -p "$base" -m "chore(release): prepare v$version")"
  unset GIT_INDEX_FILE
  rm -f "$index"
  if [[ "$dry" == 1 ]]; then
    git --no-pager diff --stat "$base" "$commit"
    git --no-pager show "$commit:RELEASE_NOTES.md"
    echo "dry run: would push $commit to main; v$version is tagged once a commit from it on passes its checks"
    return
  fi
  git push --quiet --no-verify origin "$commit:refs/heads/main" || return 1
  echo "v$version bump pushed as $commit; it is tagged once a commit from it on passes its checks"
}

tag() {
  local dry=0 notes_file="" step version sha
  while (($#)); do
    case "$1" in
      --dry-run) dry=1 ;;
      --notes) notes_file="$2"; shift ;;
      *) echo "Unknown option: $1" >&2; exit 2 ;;
    esac
    shift
  done
  for _ in 1 2 3; do
    fetch
    step="$(next_step)"
    case "$step" in
      bump*)
        bump "${step#bump }" "$dry" "$notes_file" && return ;;
      tag*)
        read -r _ version sha <<<"$step"
        if ((dry)); then echo "dry run: would tag v$version on $sha"; return; fi
        git tag "v$version" "$sha"
        git push --quiet --no-verify origin "v$version"
        [[ "$(git ls-remote origin "refs/tags/v$version" | cut -f1)" == "$sha" ]] || { echo "v$version did not reach origin" >&2; exit 1; }
        echo "v$version tagged on $sha"
        return ;;
      *) echo "${step#wait: }" >&2; exit 1 ;;
    esac
  done
  echo "main kept moving; run it again" >&2
  exit 1
}

case "${1:-}" in
  check) check ;;
  why) fetch && next_step ;;
  tag) shift && tag "$@" ;;
  hold) mkdir -p "$(dirname "$HOLD")" && echo "${2:-by hand} since $(date '+%F %R')" >"$HOLD" ;;
  unhold) rm -f "$HOLD" ;;
  *) echo "Usage: $0 check|why|tag [--dry-run] [--notes FILE]|hold [REASON]|unhold" >&2; exit 2 ;;
esac
