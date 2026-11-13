#!/usr/bin/env bash
# The Rust gates `pnpm check` and CI both run, spelled only here so they can't differ.
# CI's nextest stands in for `pnpm rust:test` on purpose: it runs test binaries at once.
set -euo pipefail

cd "$(dirname "$0")/.."

# A bare cargo run leaves out the GPUI app and its crates.
gpui=(-p sikemux-gpui -p sikemux-app)
for crate in app/crates/*/; do
  gpui+=(-p "$(basename "$crate")")
done

case "${1:-}" in
  fmt) cargo fmt --all -- --check ;;
  hakari) cargo hakari generate --diff ;;
  clippy) cargo clippy --all-targets --all-features -- -D warnings ;;
  no-plugins) cargo clippy -p sikemux-host --no-default-features --all-targets -- -D warnings ;;
  # CI's tests run as two jobs so each compiles the core once: its unit tests in one, and in the
  # other every remaining target, which between them is `--all-targets --all-features`.
  core-unit-tests) cargo nextest run -p sikemux-core --all-features --lib ;;
  other-tests)
    members=(--workspace "${gpui[@]/#-p/--exclude}")
    # The core's features go by name where the core is left out, or it would be built again without them.
    core_features="$(sed -n '/^\[features\]$/,/^\[/s|^\([a-z0-9-]*\) *=.*|sikemux-core/\1|p' crates/sikemux-core/Cargo.toml | paste -sd, -)"
    cargo nextest run "${members[@]}" --exclude sikemux-core --all-features --features "$core_features" --lib
    targets=(--bins --examples --test '*')
    if [ -n "$(find crates plugins -maxdepth 2 -name benches)" ]; then targets+=(--bench '*'); fi
    cargo nextest run "${members[@]}" --all-features "${targets[@]}"
    ;;
  gpui-clippy) cargo clippy "${gpui[@]}" --all-targets -- -D warnings ;;
  gpui-test) cargo test "${gpui[@]}" --all-targets ;;
  *)
    echo "usage: $0 fmt|hakari|clippy|no-plugins|core-unit-tests|other-tests|gpui-clippy|gpui-test" >&2
    exit 2
    ;;
esac
