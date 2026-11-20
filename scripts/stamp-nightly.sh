#!/usr/bin/env bash
# Gives the checked-out commit of main its nightly version and notes, and prints the version.
# The version is the next stable release's, then -nightly. and the count of commits since the
# last stable release left main, so every commit has its own and later commits have higher ones.
set -euo pipefail

cd "$(dirname "$0")/.."
REPO=nodelike/sikemux

from="$(node -p 'require("./package.json").version')"
core="${from%%-*}"
if git rev-parse -q --verify "refs/tags/v$core" >/dev/null; then
  IFS=. read -r major minor patch <<<"$core"
  core="$major.$minor.$((patch + 1))"
fi
stable="$(git tag -l 'v[0-9]*' --sort=-v:refname | grep -v -- - | head -1)"
version="$core-nightly.$(git rev-list --count "$(git merge-base HEAD "$stable")..HEAD")"
last="$(git tag -l 'v*-nightly.*' --merged HEAD --sort=-v:refname | head -1)"

scripts/set-version.sh "$version"

{
  printf '# Sikemux v%s\n\n' "$version"
  # shellcheck disable=SC2016
  printf 'Built from [`%s`](https://github.com/%s/commit/%s) on main.\n' "$(git rev-parse --short HEAD)" "$REPO" "$(git rev-parse HEAD)"
  if [[ -n "$last" ]]; then
    printf '\n## New since %s\n\n' "${last#v}"
    git log --reverse --no-merges --format=%s "$last..HEAD" |
      grep -E '^(feat|fix|perf)(\([^)]*\))?!?: ' |
      sed -E 's/^[a-z]+(\(([^)]*)\))?!?: (.*)$/- \3/; s/^(- [^.]*\.)( .*)?$/\1/' || true
    printf "\nFor the complete patch history, compare [\`%s...v%s\`](https://github.com/%s/compare/%s...v%s).\n" \
      "$last" "$version" "$REPO" "$last" "$version"
  fi
} >RELEASE_NOTES.md

echo "$version"
