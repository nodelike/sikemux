#!/usr/bin/env bash
# Fails when a Rust file the given commits change is not rustfmt-formatted as the commit has it.
#   check-rustfmt.sh <revisions as git log takes them>
# It checks the newest commit's copy of each file, so it works whatever the checkout holds.
set -euo pipefail

tip="$(git rev-list -1 "$@")" || exit 0
[ -n "$tip" ] || exit 0
files=()
while IFS= read -r file; do
  git cat-file -e "$tip:$file" 2>/dev/null && files+=("$file")
done < <(git log --format= --name-only --diff-filter=AMR "$@" -- '*.rs' | sort -u)
[ "${#files[@]}" -gt 0 ] || exit 0

rustfmt="$(rustup which rustfmt 2>/dev/null || command -v rustfmt || true)"
[ -n "$rustfmt" ] || { echo "! rustfmt is not installed; Rust formatting is left to CI" >&2; exit 0; }
dir="$(cd "$(mktemp -d)" && pwd -P)"
trap 'rm -rf "$dir"' EXIT
# rustfmt follows a file's `mod` lines into its folder, so each file comes with the folder that holds it.
folders=()
while IFS= read -r folder; do folders+=("$folder"); done < <(for file in "${files[@]}"; do
  folder="$(dirname "$file")"
  if [ "$folder" = . ]; then echo "$file"; else echo "$folder"; fi
done | sort -u)
git archive "$tip" -- "${folders[@]}" | tar -x -C "$dir"
if ! (cd "$dir" && "$rustfmt" --edition 2021 --check "${files[@]}" >"$dir/.diff" 2>&1); then
  printf '\033[31m✗ rustfmt would change these files; run cargo fmt and amend:\033[0m\n'
  grep -E '^Diff in |^Error' "$dir/.diff" | sed "s|$dir/||" | sort -u >&2
  exit 1
fi
