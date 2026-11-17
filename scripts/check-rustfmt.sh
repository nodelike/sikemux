#!/usr/bin/env bash
# The one Rust format check, shared by pre-push and the land queue's landChecks.
#   check-rustfmt.sh <revisions as git log takes them>   the newest commit's copy of each Rust file they change
#   check-rustfmt.sh --files [--write] <file>...         those files as the checkout has them
#   check-rustfmt.sh --stdin <file>                      formats that file's text from stdin to stdout
# Each file is formatted with its crate's edition, read from the nearest Cargo.toml.
set -euo pipefail

rustfmt="$(rustup which rustfmt 2>/dev/null || command -v rustfmt || true)"
[ -n "$rustfmt" ] || [ "${1:-}" != --stdin ] || exec cat
[ -n "$rustfmt" ] || { echo "! rustfmt is not installed; Rust formatting is left to CI" >&2; exit 0; }

edition_of() {
  local dir
  dir="$(dirname "$1")"
  while :; do
    if [ -f "$dir/Cargo.toml" ] && grep -q '^\[package\]' "$dir/Cargo.toml"; then
      sed -n 's/^edition *= *"\([0-9]*\)".*/\1/p' "$dir/Cargo.toml" | head -1 | grep . || echo 2015
      return
    fi
    [ "$dir" = . ] || [ "$dir" = / ] && break
    dir="$(dirname "$dir")"
  done
  echo 2021
}

# Formats or checks the files in the current folder; prints each file rustfmt would change.
format() {
  local write=$1 edition
  shift
  local -a mode=(--check)
  [ "$write" = 1 ] && mode=()
  local status=0
  for edition in $(for file; do edition_of "$file"; done | sort -u); do
    local -a group=()
    for file; do [ "$(edition_of "$file")" = "$edition" ] && group+=("$file"); done
    if ! out="$("$rustfmt" --edition "$edition" ${mode[@]+"${mode[@]}"} "${group[@]}" 2>&1)"; then
      status=1
      printf '%s\n' "$out" | grep -E '^Diff in |^Error' | sed -E 's/:[0-9]+:$//' | sort -u
    fi
  done
  return $status
}

refuse() {
  echo '✗ rustfmt would change these files; run cargo fmt and commit:' >&2
  sed "s|$1/||" >&2
  exit 1
}

if [ "${1:-}" = --stdin ]; then
  exec "$rustfmt" --edition "$(edition_of "$2")" --emit stdout
fi

if [ "${1:-}" = --files ]; then
  shift
  write=0
  if [ "${1:-}" = --write ]; then write=1; shift; fi
  files=()
  for file; do [[ "$file" == *.rs && -f "$file" ]] && files+=("$file"); done
  [ "${#files[@]}" -gt 0 ] || exit 0
  report="$(format "$write" "${files[@]}")" || { [ "$write" = 1 ] && exit 1; refuse "$(pwd -P)" <<<"$report"; }
  exit 0
fi

tip="$(git rev-list -1 "$@")" || exit 0
[ -n "$tip" ] || exit 0
files=()
while IFS= read -r file; do
  git cat-file -e "$tip:$file" 2>/dev/null && files+=("$file")
done < <(git log --format= --name-only --diff-filter=AMR "$@" -- '*.rs' | sort -u)
[ "${#files[@]}" -gt 0 ] || exit 0

dir="$(cd "$(mktemp -d)" && pwd -P)"
trap 'rm -rf "$dir"' EXIT
# rustfmt follows a file's `mod` lines into its folder, so each file comes with the folder that holds it.
folders=()
while IFS= read -r folder; do folders+=("$folder"); done < <(for file in "${files[@]}"; do
  folder="$(dirname "$file")"
  if [ "$folder" = . ]; then echo "$file"; else echo "$folder"; fi
done | sort -u)
git archive "$tip" -- "${folders[@]}" ':(glob)**/Cargo.toml' | tar -x -C "$dir"
report="$(cd "$dir" && format 0 "${files[@]}")" || refuse "$dir" <<<"$report"
