#!/usr/bin/env bash
# Prints the newest failed push run on main for each workflow, so a watcher fires once per new red.
# When GitHub can't be reached it prints the last answer, so an outage doesn't look like a change.
set -euo pipefail

last="${TMPDIR:-/tmp}/sikemux-ci-fan-last"
out=""
for workflow in ci.yml mobile-ci.yml server.yml docs.yml; do
  if ! line=$(gh run list --branch main --event push --workflow "$workflow" \
    --status failure --limit 1 --json workflowName,headSha,databaseId \
    --jq '.[] | "\(.workflowName) \(.headSha[0:12]) run \(.databaseId)"' 2>/dev/null); then
    cat "$last" 2>/dev/null || true
    exit 0
  fi
  out+="${line:+$line$'\n'}"
done
printf '%s' "$out" | tee "$last"
