#!/usr/bin/env bash
# shellcheck disable=SC2154 # GitHub sets the runner's variables.
# Waits for an artifact that another job of this workflow run uploads, then unpacks it.
# GitHub's API answers 404 now and then, so a failed call is tried again until the deadline.
# Gives up as soon as one of the named jobs ends without success.
# Usage: wait-for-artifact.sh <artifact> <directory> <job name>...
set -euo pipefail

name="$1" dir="$2"
shift 2
run="repos/$GITHUB_REPOSITORY/actions/runs/$GITHUB_RUN_ID"
deadline=$((SECONDS + 75 * 60))

mkdir -p "$dir"
while :; do
  id="$(gh api "$run/artifacts?name=$name" --jq '.artifacts[0].id // empty' || true)"
  if [[ -n "$id" ]] && gh api "repos/$GITHUB_REPOSITORY/actions/artifacts/$id/zip" >"$dir/$name.zip"; then
    break
  fi
  for job in "$@"; do
    conclusion="$(gh api --paginate "$run/jobs" --jq ".jobs[] | select(.name == \"$job\") | .conclusion // empty" || true)"
    if [[ -n "$conclusion" && "$conclusion" != success ]]; then
      echo "::error::$job ended with $conclusion, so $name will never arrive" >&2
      exit 1
    fi
  done
  ((SECONDS < deadline)) || { echo "::error::$name did not arrive within 75 minutes" >&2; exit 1; }
  sleep 10
done

unzip -q -o "$dir/$name.zip" -d "$dir"
rm "$dir/$name.zip"
echo "Fetched $name after ${SECONDS}s"
