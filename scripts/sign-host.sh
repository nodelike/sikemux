#!/usr/bin/env bash
# Sign the Linux host cores and the AppImage with the updater key, so a host or
# an AppImage updating itself installs only what Sikemux signed. It runs only a pinned Tauri CLI, never
# the build's dependencies, and never runs what it signs.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd -P)"
VERSION="${1:-}"
TAURI_CLI_VERSION=2.12.0
TAURI_CLI_SHA256=8544e19c4312f653e80f92241ee4212b4d928ad33461bfb1d2997c52560c418f

fail() {
  echo "Host signing failed: $*" >&2
  exit 1
}

[[ -n "$VERSION" ]] || { echo "Usage: $0 <version>" >&2; exit 2; }
[[ "$(uname -s)-$(uname -m)" == "Linux-x86_64" ]] || fail "Linux x86_64 is required"
[[ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ]] || fail "TAURI_SIGNING_PRIVATE_KEY is not set"

shopt -s nullglob
HOSTS=("$ROOT"/target/host/sikemux-host-"$VERSION"-linux-*.tar.gz)
shopt -u nullglob
[[ ${#HOSTS[@]} -eq 2 ]] || fail "expected the x86_64 and aarch64 host cores, found ${#HOSTS[@]}"
APPIMAGE="$ROOT/target/release/bundle/appimage/Sikemux_${VERSION}_amd64.AppImage"
[[ -s "$APPIMAGE" ]] || fail "missing $APPIMAGE"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
echo "→ Fetching Tauri CLI $TAURI_CLI_VERSION"
curl -fsSL --retry 3 -o "$WORK/tauri-cli.tgz" \
  "https://github.com/tauri-apps/tauri/releases/download/tauri-cli-v$TAURI_CLI_VERSION/cargo-tauri-x86_64-unknown-linux-gnu.tgz"
[[ "$(sha256sum "$WORK/tauri-cli.tgz" | cut -d ' ' -f 1)" == "$TAURI_CLI_SHA256" ]] ||
  fail "the Tauri CLI download does not match its pinned hash"
tar -xzf "$WORK/tauri-cli.tgz" -C "$WORK" cargo-tauri

for signed in "${HOSTS[@]}" "$APPIMAGE"; do
  rm -f "$signed.sig"
  "$WORK/cargo-tauri" signer sign --app-version "$VERSION" "$signed" >/dev/null
  node "$ROOT/scripts/verify-updater-signature.mjs" "$signed" "$signed.sig"
done
