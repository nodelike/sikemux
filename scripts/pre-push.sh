#!/usr/bin/env bash
# Runs the CI gates against the commits being pushed, skipping groups nothing touched.
set -uo pipefail

cd "$(dirname "$0")/.." || exit 1

# Homebrew's Rust ignores rust-toolchain.toml, so rustup's proxies must come first.
[ -x "$HOME/.cargo/bin/rustup" ] && PATH="$HOME/.cargo/bin:$PATH"

ZERO='0000000000000000000000000000000000000000'
BOLD=$'\033[1m'
DIM=$'\033[2m'
RED=$'\033[31m'
GREEN=$'\033[32m'
YELLOW=$'\033[33m'
RESET=$'\033[0m'

FAILED=()
SKIPPED=()

pushed_files() {
  if [ -t 0 ]; then
    git diff --name-only '@{upstream}'..HEAD 2>/dev/null || git diff --name-only HEAD~1..HEAD
    return
  fi
  while read -r _ local_sha _ remote_sha; do
    [ "$local_sha" = "$ZERO" ] && continue
    if [ "$remote_sha" != "$ZERO" ] && git cat-file -e "$remote_sha^{commit}" 2>/dev/null; then
      git diff --name-only "$remote_sha" "$local_sha"
    else
      base="$(git merge-base origin/main "$local_sha" 2>/dev/null || true)"
      git diff --name-only "${base:-${local_sha}^}" "$local_sha" 2>/dev/null
    fi
  done
}

touches() {
  printf '%s\n' "$CHANGED" | grep -qE "$1"
}

step() {
  label="$1"
  shift
  printf '\n%s→ %s%s\n' "$BOLD" "$label" "$RESET"
  if "$@"; then
    return 0
  fi
  FAILED+=("$label")
  return 1
}

needs() {
  command -v "$1" >/dev/null 2>&1 && return 0
  SKIPPED+=("$2 — install $1")
  printf '\n%s⚠ skipping %s (%s not installed; CI still runs it)%s\n' "$YELLOW" "$2" "$1" "$RESET"
  return 1
}

rust_toolchain_matches() {
  pinned="$(sed -n 's/^channel = "\(.*\)"$/\1/p' rust-toolchain.toml)"
  active="$(rustc -V | cut -d ' ' -f 2)"
  [ "$active" = "$pinned" ] && return 0
  printf 'rustc %s is not the %s that rust-toolchain.toml pins and CI builds with; install rustup to get it\n' "$active" "$pinned"
  return 1
}

summary() {
  for entry in "${SKIPPED[@]+"${SKIPPED[@]}"}"; do
    printf '%s⚠ unverified: %s%s\n' "$YELLOW" "$entry" "$RESET"
  done
  if [ "${#FAILED[@]}" -eq 0 ]; then
    printf '\n%s✓ local CI gates passed%s\n' "$GREEN" "$RESET"
    return 0
  fi
  printf '\n%s✗ %d gate(s) failed — CI would fail too:%s\n' "$RED" "${#FAILED[@]}" "$RESET"
  for entry in "${FAILED[@]}"; do
    printf '%s    %s%s\n' "$RED" "$entry" "$RESET"
  done
  printf '%s  push anyway with: git push --no-verify%s\n' "$DIM" "$RESET"
  return 1
}

if [ "${SKIP_PREPUSH:-}" = "1" ]; then
  printf '%sSKIP_PREPUSH=1 — skipping local CI gates%s\n' "$YELLOW" "$RESET"
  exit 0
fi

CHANGED="$(pushed_files | sort -u)"
if [ "${PREPUSH_FULL:-}" = "1" ] || [ -z "$CHANGED" ]; then
  CHANGED='(full run)'
  RUST=1 FRONTEND=1 SHELL_SCRIPTS=1 RELEASE=1
else
  RUST=0 FRONTEND=0 SHELL_SCRIPTS=0 RELEASE=0
  touches '^(src-tauri/|rust-toolchain\.toml$)' && RUST=1
  touches '^(src/|public/|index\.html|package\.json|pnpm-lock\.yaml|vite\.config\.ts|eslint\.config\.js|tsconfig\.json)' && FRONTEND=1
  touches '^scripts/.*\.sh$' && SHELL_SCRIPTS=1
  touches '^(scripts/|package\.json|latest\.json|src-tauri/tauri.*\.conf\.json)' && RELEASE=1
fi

printf '%sChecking %s commits against the CI gates%s\n' "$BOLD" "$(printf '%s\n' "$CHANGED" | wc -l | tr -d ' ')" "$RESET"

# Cheap gates first, so a stray format error does not cost a full test run.
step 'prettier format' pnpm format:check
[ "$SHELL_SCRIPTS" = 1 ] && needs shellcheck 'shell lint' && step 'shell lint' shellcheck scripts/*.sh
[ "$RUST" = 1 ] && step 'rust toolchain' rust_toolchain_matches
[ "$RUST" = 1 ] && step 'cargo fmt' pnpm rust:fmt:check
[ "$RUST" = 1 ] && needs cargo-hakari 'workspace-hack' && step 'workspace-hack' pnpm rust:hakari:check
[ "$RUST" = 1 ] && needs cargo-audit 'rust security audit' && step 'rust security audit' cargo audit --file src-tauri/Cargo.lock
[ "$FRONTEND" = 1 ] && step 'eslint' pnpm lint
[ "$FRONTEND" = 1 ] && step 'typescript' pnpm typecheck
[ "$FRONTEND" = 1 ] && step 'ipc contracts' pnpm ipc:check
[ "$FRONTEND" = 1 ] && step 'grammar manifest' pnpm grammars:check

if [ "${#FAILED[@]}" -ne 0 ]; then
  summary
  exit 1
fi

[ "$RUST" = 1 ] && step 'clippy' pnpm rust:clippy
[ "$RUST" = 1 ] && step 'rust tests' pnpm rust:test
# Several agents share this checkout, and a coverage run wipes its own report
# directory as it starts. Two of them pointed at the same one take turns
# deleting each other's half-written files, which kills a worker and fails a
# test that has nothing wrong with it. The gate only wants a pass or a fail,
# so it keeps its report somewhere of its own and throws it away after.
if [ "$FRONTEND" = 1 ]; then
  COVERAGE_DIR="$(mktemp -d)"
  trap 'rm -rf "$COVERAGE_DIR"' EXIT
  step 'frontend tests' pnpm test:coverage --coverage.reportsDirectory="$COVERAGE_DIR"
fi
[ "$FRONTEND" = 1 ] && step 'frontend build' pnpm build
[ "$FRONTEND" = 1 ] && step 'performance budget' pnpm perf:budget
[ "$RELEASE" = 1 ] && step 'release tooling' pnpm release:check

summary
