#!/usr/bin/env bash
set -uo pipefail

jobs="repos/${GITHUB_REPOSITORY:?}/actions/runs/${GITHUB_RUN_ID:?}/attempts/${GITHUB_RUN_ATTEMPT:?}/jobs?per_page=100"
checks='Frontend quality, build, and tests|Rust formatting and workspace-hack|Linux compile check and core tests'
failed="${RUNNER_TEMP:?}/failed-linux-checks"

state() {
  gh api "$jobs" --jq "[.jobs[] | select(.name | test(\"$checks\"))]
    | if any(.conclusion == \"failure\") then (\"failed\", (.[] | select(.conclusion == \"failure\") | .name))
      elif all(.status == \"completed\") then \"passed\" else \"waiting\" end" ||
    echo waiting
}

stop_tree() {
  local child
  for child in $(pgrep -P "$1"); do stop_tree "$child"; done
  kill -TERM "$1" 2>/dev/null
}

if [ "$1" = once ]; then
  result="$(state)"
  [ "$(head -1 <<<"$result")" = failed ] || exit 0
  tail -n +2 <<<"$result" >"$failed"
  exit 1
fi

while sleep 30; do
  result="$(state)"
  case "$(head -1 <<<"$result")" in
    passed) exit 0 ;;
    failed) break ;;
  esac
done
tail -n +2 <<<"$result" >"$failed"
for worker in $(pgrep -f Runner.Worker); do
  for child in $(pgrep -P "$worker"); do stop_tree "$child"; done
done
