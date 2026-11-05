#!/usr/bin/env bash
# Lists the commits the given revisions select, as git log takes them, that credit
# Claude, and fails if there are any. Commits here credit only people.
set -uo pipefail
# shellcheck source=/dev/null
source "$(dirname "$0")/credit-pattern.sh"

credited="$(
  git log --format='commit %h %s%n%B' "$@" |
    awk -v credit="$CLAUDE_CREDIT" '/^commit [0-9a-f]+ / { commit = substr($0, 8); next } tolower($0) ~ credit && commit != "" { print "  " commit; commit = "" }'
)"
[ -z "$credited" ] && exit 0
printf '\033[31m✗ these commits credit Claude, and commits here credit only people:\033[0m\n%s\n' "$credited"
printf '\033[2m  reword them without the Co-Authored-By / Generated with Claude Code lines\033[0m\n'
exit 1
