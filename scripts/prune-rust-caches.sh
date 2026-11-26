#!/usr/bin/env bash
# Keep only the newest Swatinem/rust-cache entry per (job, os, arch) on main.
# When Cargo.lock changes, prefix-fallback restores bump the old cache's last-accessed
# time so GitHub's 10 GiB LRU sweeper evicts Release's sccache instead of the dead cache.
set -euo pipefail

: "${GH_TOKEN:?GH_TOKEN is required}"
: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"

caches="$(gh api --paginate "repos/$GITHUB_REPOSITORY/actions/caches?key=v0-rust-&ref=refs/heads/main&per_page=100" --jq '.actions_caches[]' | jq -s '.')"

stale="$(jq -r '
  group_by(.key | sub("-[0-9a-f]{8}(-[0-9a-f]{8})?$"; ""))
  | map(sort_by(.created_at) | reverse | .[1:][])
  | .[]
  | "\(.id)\t\(.key)\t\(.size_in_bytes)"
' <<<"$caches")"

[[ -n "$stale" ]] || exit 0

while IFS=$'\t' read -r id key bytes; do
  if gh api -X DELETE "repos/$GITHUB_REPOSITORY/actions/caches/$id" >/dev/null 2>&1; then
    echo "Pruned superseded cache $key ($((bytes / 1048576)) MiB)"
  fi
done <<<"$stale"
