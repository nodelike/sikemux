#!/usr/bin/env bash
# Sign the Linux host cores and the AppImages, or with `windows` the Windows installers, with the
# updater key, so whatever updates itself installs only what Sikemux signed. It runs only a pinned
# Tauri CLI, never the build's dependencies, and never runs what it signs.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd -P)"
VERSION="${1:-}"
SET="${2:-linux}"
# shellcheck source=scripts/release-shape.sh
source "$ROOT/scripts/release-shape.sh"
TAURI_CLI_SHA256=8544e19c4312f653e80f92241ee4212b4d928ad33461bfb1d2997c52560c418f

fail() {
  echo "Host signing failed: $*" >&2
  exit 1
}

[[ -n "$VERSION" ]] || { echo "Usage: $0 <version> [windows]" >&2; exit 2; }
[[ "$(uname -s)-$(uname -m)" == "Linux-x86_64" ]] || fail "Linux x86_64 is required"
[[ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ]] || fail "TAURI_SIGNING_PRIVATE_KEY is not set"

shopt -s nullglob
if [[ "$SET" == windows ]]; then
  SIGNED=("$ROOT"/target/windows/Sikemux_"$VERSION"_*-setup.exe)
  [[ ${#SIGNED[@]} -eq 2 ]] || fail "expected the x64 and arm64 Windows installers, found ${#SIGNED[@]}"
else
  HOSTS=("$ROOT"/target/host/sikemux-host-"$VERSION"-linux-*.tar.gz)
  APPIMAGES=("$ROOT"/target/release/bundle/appimage/Sikemux_"$VERSION"_*.AppImage)
  [[ ${#HOSTS[@]} -eq 2 ]] || fail "expected the x86_64 and aarch64 host cores, found ${#HOSTS[@]}"
  [[ ${#APPIMAGES[@]} -eq 2 ]] || fail "expected the amd64 and arm64 AppImages, found ${#APPIMAGES[@]}"
  SIGNED=("${HOSTS[@]}" "${APPIMAGES[@]}")
fi
shopt -u nullglob

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
echo "→ Fetching Tauri CLI $TAURI_CLI_VERSION"
curl -fsSL --retry 3 -o "$WORK/tauri-cli.tgz" \
  "https://github.com/tauri-apps/tauri/releases/download/tauri-cli-v$TAURI_CLI_VERSION/cargo-tauri-x86_64-unknown-linux-gnu.tgz"
[[ "$(sha256sum "$WORK/tauri-cli.tgz" | cut -d ' ' -f 1)" == "$TAURI_CLI_SHA256" ]] ||
  fail "the Tauri CLI download does not match its pinned hash"
tar -xzf "$WORK/tauri-cli.tgz" -C "$WORK" cargo-tauri

for signed in "${SIGNED[@]}"; do
  rm -f "$signed.sig"
  "$WORK/cargo-tauri" signer sign --app-version "$VERSION" "$signed" >/dev/null
  node "$ROOT/scripts/verify-updater-signature.mjs" "$signed" "$signed.sig"
done
