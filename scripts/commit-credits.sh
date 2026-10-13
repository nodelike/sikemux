#!/usr/bin/env bash
# Lists the commits in the given ranges that credit Claude, and fails if there are any.
# Commits here credit only people.
set -uo pipefail

credited="$(
  for range in "$@"; do
    git log --format='commit %h %s%n%B' "$range" |
      awk '/^commit [0-9a-f]+ / { commit = substr($0, 8); next } tolower($0) ~ /^co-authored-by:.*(claude|anthropic\.com)|generated with \[?claude code/ && commit != "" { print "  " commit; commit = "" }'
  done
)"
[ -z "$credited" ] && exit 0
printf '\033[31m✗ these commits credit Claude, and commits here credit only people:\033[0m\n%s\n' "$credited"
printf '\033[2m  reword them without the Co-Authored-By / Generated with Claude Code lines\033[0m\n'
exit 1
