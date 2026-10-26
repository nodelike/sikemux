#!/usr/bin/env bash
# Build the x86_64 Windows app as a zip: the GPUI app with its core beside it, unsigned.
# Run from Git Bash with the Visual Studio C++ build tools installed.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd -P)"
VERSION="$(node -p "require('./package.json').version")"

fail() {
  echo "Windows packaging failed: $*" >&2
  exit 1
}

[[ "$(uname -s)" == MINGW* || "$(uname -s)" == MSYS* ]] || fail "run this from Git Bash on Windows"
for command in cargo node powershell; do
  command -v "$command" >/dev/null || fail "missing required command: $command"
done

(cd src-tauri && cargo build --release --locked -p sikemux-host)
(cd src-tauri/crates/sikemux-gpui && cargo build --release --locked --bin sikemux)

WORK="$ROOT/src-tauri/target/release/windows"
BUNDLE="$ROOT/src-tauri/target/release/bundle/windows"
rm -rf "$WORK" "$BUNDLE"
mkdir -p "$WORK/Sikemux" "$BUNDLE"
cp "$ROOT/src-tauri/crates/sikemux-gpui/target/release/sikemux.exe" "$WORK/Sikemux/sikemux.exe"
cp "$ROOT/src-tauri/target/release/sikemux-host.exe" "$WORK/Sikemux/sikemux-host.exe"
cp "$ROOT/src-tauri/resources/sikemux_pi_tools.ts" "$WORK/Sikemux/sikemux_pi_tools.ts"

ZIP="$BUNDLE/Sikemux_${VERSION}_x64-windows.zip"
powershell -NoProfile -Command "Compress-Archive -Path '$(cygpath -w "$WORK/Sikemux")' -DestinationPath '$(cygpath -w "$ZIP")'"
echo "$ZIP"
