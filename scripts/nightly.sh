#!/usr/bin/env bash
# Tags the next nightly from origin/main, as docs/releasing.md describes.
#   nightly.sh check            prints the next nightly's version once one is due, else the last answer
#   nightly.sh tag [--dry-run] [--notes FILE]
# A nightly is due when main has commits since the last nightly, that nightly is at least
# SIKEMUX_NIGHTLY_HOURS (12) old, every push workflow's newest finished run on main passed
# and no Release run is going. tag commits the version bump and notes on origin/main's tip
# from a temporary index, so no checkout moves, then pushes it and the tag that starts Release.
set -euo pipefail

cd "$(dirname "$0")/.."
REPO=nodelike/sikemux
HOURS="${SIKEMUX_NIGHTLY_HOURS:-12}"
WORKFLOWS=(ci.yml mobile-ci.yml server.yml docs.yml)

fetch() {
  git fetch --quiet origin main
  git fetch --quiet --tags --force origin
}

last_tag() {
  git tag -l 'v*-nightly.*' --sort=-v:refname | head -1
}

next_version() {
  local current core n
  current="$(git show origin/main:package.json | node -pe 'JSON.parse(require("fs").readFileSync(0)).version')"
  core="${current%%-*}"
  if git rev-parse -q --verify "refs/tags/v$core" >/dev/null; then
    IFS=. read -r major minor patch <<<"$core"
    echo "$major.$minor.$((patch + 1))-nightly.1"
  elif [[ "$current" != *-nightly.* ]]; then
    echo "$core-nightly.1"
  elif git rev-parse -q --verify "refs/tags/v$current" >/dev/null; then
    n="${current##*.}"
    echo "$core-nightly.$((n + 1))"
  else
    echo "$current"
  fi
}

why_not_due() {
  local last age
  last="$(last_tag)"
  [[ -n "$(git rev-list -1 "$last..origin/main")" ]] || { echo "main has nothing since $last"; return; }
  age=$(($(date +%s) - $(git log -1 --format=%ct "$last^{commit}")))
  ((age >= HOURS * 3600)) || { echo "$last is only $((age / 3600))h old"; return; }
  for workflow in "${WORKFLOWS[@]}"; do
    local result
    result="$(gh run list -R "$REPO" --branch main --event push --workflow "$workflow" --status completed \
      --limit 10 --json conclusion --jq '[.[] | select(.conclusion != "cancelled" and .conclusion != "skipped")][0].conclusion // "success"')"
    [[ "$result" == success ]] || { echo "$workflow's newest run on main ended $result"; return; }
  done
  [[ "$(gh run list -R "$REPO" --workflow release.yml --limit 5 --json status \
    --jq '[.[] | select(.status != "completed")] | length')" == 0 ]] || echo "a Release run is going"
}

check() {
  local answer="${TMPDIR:-/tmp}/sikemux-nightly-last"
  if fetch 2>/dev/null && [[ -z "$(why_not_due)" ]]; then
    next_version | tee "$answer"
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

tag() {
  local dry=0 notes_file=""
  while (($#)); do
    case "$1" in
      --dry-run) dry=1 ;;
      --notes) notes_file="$2"; shift ;;
      *) echo "Unknown option: $1" >&2; exit 2 ;;
    esac
    shift
  done
  local index
  index="$(mktemp)"
  trap 'rm -f "$index"' RETURN
  for _ in 1 2 3; do
    fetch
    local base version from last bump
    base="$(git rev-parse origin/main)"
    version="$(next_version)"
    from="$(git show "$base:package.json" | node -pe 'JSON.parse(require("fs").readFileSync(0)).version')"
    last="$(last_tag)"
    if git rev-parse -q --verify "refs/tags/v$version" >/dev/null; then
      echo "v$version exists already" >&2
      exit 1
    fi
    put() { git update-index --add --cacheinfo "100644,$(git hash-object -w --stdin),$1"; }
    export GIT_INDEX_FILE="$index"
    git read-tree "$base"
    git show "$base:package.json" | sed "s/\"version\": \"$from\"/\"version\": \"$version\"/" | put package.json
    for file in Cargo.toml Cargo.lock; do
      git show "$base:$file" | sed "s/^version = \"$from\"$/version = \"$version\"/" | put "$file"
    done
    if [[ -n "$notes_file" ]]; then put RELEASE_NOTES.md <"$notes_file"; else notes "$version" "$last" | put RELEASE_NOTES.md; fi
    bump="$(git commit-tree "$(git write-tree)" -p "$base" -m "chore(release): prepare v$version")"
    unset GIT_INDEX_FILE
    if ((dry)); then
      git --no-pager diff --stat "$base" "$bump"
      git --no-pager show "$bump:RELEASE_NOTES.md"
      echo "dry run: would push $bump to main and tag v$version"
      return
    fi
    if git push --quiet --no-verify origin "$bump:refs/heads/main"; then
      git tag "v$version" "$bump"
      git push --quiet --no-verify origin "v$version"
      [[ "$(git ls-remote origin "refs/tags/v$version" | cut -f1)" == "$bump" ]] || { echo "v$version did not reach origin" >&2; exit 1; }
      echo "v$version tagged on $bump"
      return
    fi
  done
  echo "main kept moving; run it again" >&2
  exit 1
}

case "${1:-}" in
  check) check ;;
  why) fetch && why_not_due ;;
  tag) shift && tag "$@" ;;
  *) echo "Usage: $0 check|why|tag [--dry-run] [--notes FILE]" >&2; exit 2 ;;
esac
