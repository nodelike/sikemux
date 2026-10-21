#!/usr/bin/env bash
# Installs the host core a release is about to publish with the installer it
# publishes beside it, from local files laid out as the release will be, so the
# one-liner can never ship pointing at files it does not match.
set -euo pipefail

cd "$(dirname "$0")/.."
ARCH="${1:?usage: $0 <x86_64|aarch64>}"
VERSION="$(sed -n 's/^  "version": "\(.*\)",$/\1/p' package.json)"
ASSET="src-tauri/target/host/sikemux-host-${VERSION}-linux-${ARCH}.tar.gz"

fail() {
  echo "Host install smoke failed: $*" >&2
  exit 1
}

[[ -f "$ASSET" ]] || fail "$ASSET is not packed"
RELEASE="$(mktemp -d)"
trap 'rm -rf "$RELEASE"' EXIT
mkdir -p "$RELEASE/releases/download/nightly" "$RELEASE/releases/download/v$VERSION"
printf '{\n  "version": "%s"\n}\n' "$VERSION" >"$RELEASE/releases/download/nightly/latest.json"
cp "$ASSET" "$RELEASE/releases/download/v$VERSION/"
sed "s|^REPO=.*|REPO=\"file://$RELEASE\"|" scripts/install.sh >"$RELEASE/releases/download/v$VERSION/install.sh"

install_once() {
  SIKEMUX_CHANNEL=nightly sh "$RELEASE/releases/download/v$VERSION/install.sh" --host --yes </dev/null ||
    fail "the installer exited with $?"
  installed="$(sikemux --version)" || fail "sikemux does not run after installing"
  [[ "$installed" == "sikemux $VERSION" ]] || fail "installed $installed, not $VERSION"
}

export PATH="/usr/local/bin:$HOME/.local/bin:$PATH"
install_once
install_once
sikemux status >/dev/null || [[ $? -eq 3 ]] || fail "sikemux status failed"
echo "✓ installed v$VERSION twice with its own installer"
