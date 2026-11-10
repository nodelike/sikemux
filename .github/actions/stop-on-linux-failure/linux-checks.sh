#!/usr/bin/env bash
set -uo pipefail

jobs="repos/${GITHUB_REPOSITORY:?}/actions/runs/${GITHUB_RUN_ID:?}/attempts/${GITHUB_RUN_ATTEMPT:?}/jobs?per_page=100"
checks='Frontend quality, build, and tests|Rust formatting and workspace-hack|Linux compile check and core tests'

state() {
  gh api "$jobs" --jq "[.jobs[] | select(.name | test(\"$checks\"))]
    | if any(.conclusion == \"failure\") then \"failed\" elif all(.status == \"completed\") then \"passed\" else \"waiting\" end" ||
    echo waiting
}

stop_tree() {
  local child
  for child in $(pgrep -P "$1"); do stop_tree "$child"; done
  kill -TERM "$1" 2>/dev/null
}

if [ "$1" = once ]; then
  [ "$(state)" != failed ] || { echo "::error::A Linux check of this run failed, so this job stops"; exit 1; }
  exit 0
fi

while sleep 30; do
  case "$(state)" in
    passed) exit 0 ;;
    failed) break ;;
  esac
done
echo "A Linux check of this run failed; stopping the job's running step"
for worker in $(pgrep -f Runner.Worker); do
  for child in $(pgrep -P "$worker"); do stop_tree "$child"; done
done
