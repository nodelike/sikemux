#!/usr/bin/env bash
# Write the sikemux cask for a stable release's DMG and push it to nodelike/homebrew-tap.
# HOMEBREW_TAP_TOKEN must be able to push to that repository.
set -euo pipefail

VERSION="${1:-}"
DMG="${2:-}"
TAP=nodelike/homebrew-tap

fail() {
  echo "Homebrew push failed: $*" >&2
  exit 1
}

[[ -n "$VERSION" && -f "$DMG" ]] || { echo "Usage: $0 <version> <dmg>" >&2; exit 2; }
[[ -n "${HOMEBREW_TAP_TOKEN:-}" ]] || fail "HOMEBREW_TAP_TOKEN is not set"
case "$VERSION" in
  *-*) fail "only stable releases go to Homebrew, not $VERSION" ;;
esac
[[ "${DMG##*/}" == "Sikemux_${VERSION}_aarch64.dmg" ]] || fail "$DMG is not the $VERSION DMG"

SHA256="$(shasum -a 256 "$DMG" | cut -d' ' -f1)"
CLONE="$(mktemp -d)"
trap 'rm -rf "$CLONE"' EXIT

git clone --quiet --depth 1 "https://x-access-token:${HOMEBREW_TAP_TOKEN}@github.com/${TAP}.git" "$CLONE"
mkdir -p "$CLONE/Casks"
cat >"$CLONE/Casks/sikemux.rb" <<CASK
cask "sikemux" do
  version "$VERSION"
  sha256 "$SHA256"

  url "https://github.com/nodelike/sikemux/releases/download/v#{version}/Sikemux_#{version}_aarch64.dmg"
  name "Sikemux"
  desc "Native workspace for Claude Code, Codex and other coding agents"
  homepage "https://sikemux.com/"

  livecheck do
    url :url
    strategy :github_latest
  end

  auto_updates true
  depends_on arch: :arm64
  depends_on :macos

  app "Sikemux.app"

  zap launchctl: "com.nodelike.sikemux.core",
      trash:     [
        "~/.config/sikemux",
        "~/Library/Application Support/com.nodelike.sikemux",
        "~/Library/Caches/com.nodelike.sikemux",
        "~/Library/Caches/com.nodelike.sikemux.notch",
        "~/Library/HTTPStorages/com.nodelike.sikemux",
        "~/Library/HTTPStorages/com.nodelike.sikemux.binarycookies",
        "~/Library/LaunchAgents/com.nodelike.sikemux.core.plist",
        "~/Library/Logs/com.nodelike.sikemux",
        "~/Library/Preferences/com.nodelike.sikemux.notch.plist",
        "~/Library/Preferences/com.nodelike.sikemux.plist",
        "~/Library/WebKit/com.nodelike.sikemux",
      ]
end
CASK

cd "$CLONE"
git add Casks/sikemux.rb
if git diff --cached --quiet; then
  echo "✓ The tap already has sikemux $VERSION"
  exit 0
fi
git -c user.name="github-actions[bot]" -c user.email="41898282+github-actions[bot]@users.noreply.github.com" commit --quiet -m "sikemux $VERSION"
git push --quiet origin HEAD
echo "✓ Pushed sikemux $VERSION to $TAP"
