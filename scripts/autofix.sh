#!/usr/bin/env bash
# shellcheck disable=SC2329 # step runs its functions by name
# Applies every fix CI's checks can fail on that needs no judgement, then runs the checks.
# It only rewrites files and never stages or commits.
# Exit 0: everything passes. Exit 1: something still fails and needs a person or an agent.
set -uo pipefail

cd "$(dirname "$0")/.." || exit 2
export PATH="$PWD/scripts/bin:$PATH"

usage() {
  echo "usage: $0 [--since <commit>] [--all]" >&2
  exit 2
}

since=origin/main
all=false
while [ $# -gt 0 ]; do
  case "$1" in
    --since) [ $# -ge 2 ] || usage; since="$2"; shift 2 ;;
    --all) all=true; shift ;;
    *) usage ;;
  esac
done

logs="$(mktemp -d)"
trap 'rm -rf "$logs"' EXIT
failed=()
steps=0

changed_files() {
  { git diff --name-only "$1" --; git ls-files --others --exclude-standard; } | sort -u
}

if ! $all; then
  base="$(git merge-base "$since" HEAD 2>/dev/null)" || { echo "autofix: no common history with $since" >&2; exit 2; }
  changed="$(changed_files "$base")"
fi

touches() {
  $all || printf '%s\n' "$changed" | grep -qE "$1"
}

fingerprint() {
  changed_files HEAD | while IFS= read -r file; do
    printf '%s %s\n' "$file" "$(git hash-object "$file" 2>/dev/null || echo gone)"
  done
}
before="$(fingerprint)"

# A step's output goes to its own log, shown only when it fails.
step() {
  local name="$1"; shift
  steps=$((steps + 1))
  local log="$logs/$steps.log"
  local start=$SECONDS
  if "$@" >"$log" 2>&1; then
    printf '  ok    %-28s %ss\n' "$name" $((SECONDS - start))
  else
    printf '  FAIL  %-28s %ss\n' "$name" $((SECONDS - start))
    failed+=("$name|$log")
  fi
}

generate() {
  for generator in "$@"; do node "scripts/generate-$generator.mjs" || return 1; done
}

parallel_generate() {
  local pids=() status=0
  for chain in "wire-types agent-tools" "grammars file-icons" agent-catalog docs-reference notch-sources notch-icons faces syntax; do
    # shellcheck disable=SC2086
    generate $chain >"$logs/gen-${chain%% *}.log" 2>&1 &
    pids+=($!)
  done
  for pid in "${pids[@]}"; do wait "$pid" || status=1; done
  [ "$status" = 0 ] || cat "$logs"/gen-*.log
  return "$status"
}

# The browser and the GPUI app need GTK's Linux libraries, so only CI checks them on Linux.
# The compiler cache is left out because it slowed each C file it wrapped to many seconds.
linux_clippy() {
  RUSTC_WRAPPER='' \
    CC_aarch64_unknown_linux_gnu="$PWD/scripts/lib/zig-linux-cc" \
    AR_aarch64_unknown_linux_gnu="$PWD/scripts/lib/zig-linux-ar" \
    cargo clippy --target aarch64-unknown-linux-gnu --all-targets --all-features \
    -p sikemux-core -p sikemux-corekit -p sikemux-git -p sikemux-agents -p sikemux-kernel -p sikemux-host -p sikemux-wire -p sikemux-client -p sikemux-pty -p sikemux-process -p sikemux-keychain \
    -- -D warnings
}

rust='^(app/|crates/|plugins/|Cargo\.(toml|lock)$|\.config/hakari\.toml$|clippy\.toml$|rust-toolchain\.toml$)'
helpers='^(helpers/(notch|voice)/|scripts/build-(notch|voice)-helper\.mjs$|scripts/generate-notch-(icons|sources)\.mjs$|scripts/lib/build\.mjs$|scripts/app-metadata\.mjs$)'

echo "Fixing"
step "generated files" parallel_generate
if touches '^server/protocol/schema/' && [ -d server/node_modules ]; then
  step "server protocol" pnpm -C server protocol:generate
fi
if touches '^server/api/migrations/' && [ -d server/node_modules ]; then
  step "server schema" pnpm -C server schema:generate
fi
step "prettier" pnpm -s format
step "eslint --fix" pnpm -s exec eslint --fix --max-warnings=0 "shared/**/*.{ts,tsx}" "scripts/**/*.mjs"
if touches "$rust"; then
  if touches '(^|/)Cargo\.(toml|lock)$|^\.config/hakari\.toml$'; then
    step "hakari" pnpm -s rust:hakari
  fi
  gpui=(-p sikemux-gpui)
  for crate in app/crates/*/; do gpui+=(-p "$(basename "$crate")"); done
  step "clippy --fix" cargo clippy --fix --allow-dirty --allow-staged --all-targets --all-features
  step "clippy --fix (GPUI)" cargo clippy --fix --allow-dirty --allow-staged "${gpui[@]}" --all-targets
  step "rustfmt" cargo fmt --all
fi

echo "Checking"
step "check shared imports" node mobile/scripts/check-shared-imports.mjs
step "check types" pnpm -s typecheck
step "check shell scripts" pnpm -s shell:lint
step "check dev launcher" pnpm -s dev-launcher:test
step "check release tooling" pnpm -s release:check
if touches "$rust"; then
  step "check hakari" scripts/check-rust.sh hakari
  step "check clippy" scripts/check-rust.sh clippy
  step "check clippy GPUI" scripts/check-rust.sh gpui-clippy
  step "check clippy no plugins" scripts/check-rust.sh no-plugins
  if command -v zig >/dev/null; then
    step "check clippy Linux" linux_clippy
  else
    echo "  skip  check clippy Linux           needs zig: brew install zig"
  fi
fi
fixed="$(diff <(printf '%s\n' "$before") <(fingerprint) | sed -n 's/^> \([^ ]*\) .*/\1/p' | sort -u)"
if touches "$helpers" || printf '%s\n' "$fixed" | grep -qE '\.swift$|^helpers/'; then
  step "build notch, voice helpers" pnpm -s check:helpers
fi

echo
if [ -n "$fixed" ]; then
  echo "Rewrote:"
  printf '%s\n' "$fixed" | sed 's/^/  /'
else
  echo "Rewrote nothing."
fi

if [ ${#failed[@]} -eq 0 ]; then
  echo "Everything passes."
  exit 0
fi
echo "Still failing, not mechanical:"
for entry in "${failed[@]}"; do
  echo "--- ${entry%%|*}"
  tail -25 "${entry#*|}"
done
exit 1
