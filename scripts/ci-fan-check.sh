#!/usr/bin/env bash
# Prints each scheduled workflow's newest failed run on main, so a watcher fires once per new red.
# Pushes and pull requests are left out, since Sikemux tells the agents that made them.
# When GitHub can't be reached it prints the last answer, so an outage doesn't look like a change.
set -euo pipefail

last="${TMPDIR:-/tmp}/sikemux-ci-fan-last"
if out=$(gh run list --repo nodelike/sikemux --branch main --event schedule --status failure \
  --limit 100 --json workflowName,headSha,databaseId \
  --jq 'group_by(.workflowName)[] | max_by(.databaseId)
    | "\(.workflowName) \(.headSha[0:12]) run \(.databaseId)"' 2>/dev/null); then
  printf '%s\n' "$out" | tee "$last"
else
  cat "$last" 2>/dev/null || true
fi
