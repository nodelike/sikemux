#!/usr/bin/env bash
# Packs the core a Linux host installs (`install.sh --host`) after checking it
# starts with no window: it must not link WebKit, GTK or a display server.
set -euo pipefail

cd "$(dirname "$0")/.."
ARCH="${1:?usage: $0 <x86_64|aarch64>}"
BINARY=target/release/sikemux-host
VERSION="$(sed -n 's/^  "version": "\(.*\)",$/\1/p' package.json)"

fail() {
  echo "Host package failed: $*" >&2
  exit 1
}

[[ -x "$BINARY" ]] || fail "$BINARY is not built"
if ldd "$BINARY" | grep -Eiq 'webkit|gtk|gdk|libX|wayland|soup'; then
  ldd "$BINARY" >&2
  fail "the core links window libraries"
fi
"$BINARY" --help >/dev/null || fail "the core does not start"

OUT=target/host
mkdir -p "$OUT"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
cp "$BINARY" "$STAGE/sikemux-host"
ASSET="$OUT/sikemux-host-${VERSION}-linux-${ARCH}.tar.gz"
tar -czf "$ASSET" -C "$STAGE" sikemux-host
echo "✓ $ASSET ($(du -h "$ASSET" | cut -f1))"
