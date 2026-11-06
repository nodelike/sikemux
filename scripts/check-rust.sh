#!/usr/bin/env bash
# The Rust gates `pnpm check` and CI both run, spelled only here so they can't differ.
# CI's nextest stands in for `pnpm rust:test` on purpose: it runs test binaries at once.
set -euo pipefail

cd "$(dirname "$0")/.."

# A bare cargo run leaves out the GPUI app and its crates.
gpui=(-p sikemux-gpui)
for crate in app/crates/*/; do
  gpui+=(-p "$(basename "$crate")")
done

case "${1:-}" in
  fmt) cargo fmt --all -- --check ;;
  hakari) cargo hakari generate --diff ;;
  clippy) cargo clippy --all-targets --all-features -- -D warnings ;;
  no-plugins) cargo clippy -p sikemux-host --no-default-features --all-targets -- -D warnings ;;
  gpui-clippy) cargo clippy "${gpui[@]}" --all-targets -- -D warnings ;;
  gpui-test) cargo test "${gpui[@]}" --all-targets ;;
  *)
    echo "usage: $0 fmt|hakari|clippy|no-plugins|gpui-clippy|gpui-test" >&2
    exit 2
    ;;
esac
