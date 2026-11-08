#!/usr/bin/env bash
# Push a stable release's recipe, as packaging/arch/build-release.sh left it in <dir>,
# to the AUR's sikemux-bin. The key in AUR_SSH_PRIVATE_KEY must be on the AUR account
# that maintains it; the first push creates the package.
set -euo pipefail

VERSION="${1:-}"
DIR="${2:-}"
# The AUR's Ed25519 host key, matching the fingerprint aur.archlinux.org publishes:
# SHA256:RFzBCUItH9LZS0cKB5UE6ceAYhBD5C8GeOBip8Z11+4
AUR_HOST_KEY="aur.archlinux.org ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIEuBKrPzbawxA/k2g6NcyV5jmqwJ2s+zpgZGZ7tpLIcN"

fail() {
  echo "AUR push failed: $*" >&2
  exit 1
}

[[ -n "$VERSION" && -n "$DIR" ]] || { echo "Usage: $0 <version> <dir>" >&2; exit 2; }
[[ -n "${AUR_SSH_PRIVATE_KEY:-}" ]] || fail "AUR_SSH_PRIVATE_KEY is not set"
case "$VERSION" in
  *-*) fail "only stable releases go to the AUR, not $VERSION" ;;
esac
grep -qx "pkgname = sikemux-bin" "$DIR/.SRCINFO" || fail "$DIR does not hold the sikemux-bin recipe"
grep -qx "	pkgver = $VERSION" "$DIR/.SRCINFO" || fail "$DIR's recipe is not for $VERSION"

SSH_DIR="$(mktemp -d)"
CLONE="$(mktemp -d)"
trap 'rm -rf "$SSH_DIR" "$CLONE"' EXIT
printf '%s\n' "$AUR_SSH_PRIVATE_KEY" >"$SSH_DIR/key"
chmod 600 "$SSH_DIR/key"
printf '%s\n' "$AUR_HOST_KEY" >"$SSH_DIR/known_hosts"
export GIT_SSH_COMMAND="ssh -i $SSH_DIR/key -o IdentitiesOnly=yes -o UserKnownHostsFile=$SSH_DIR/known_hosts -o StrictHostKeyChecking=yes"

git clone --quiet ssh://aur@aur.archlinux.org/sikemux-bin.git "$CLONE"
cp "$DIR/PKGBUILD" "$DIR/.SRCINFO" "$DIR/sikemux.sysusers" "$DIR/sikemux.tmpfiles" "$CLONE/"
cd "$CLONE"
git add PKGBUILD .SRCINFO sikemux.sysusers sikemux.tmpfiles
if git diff --cached --quiet; then
  echo "✓ The AUR already has sikemux-bin $VERSION"
  exit 0
fi
git -c user.name="github-actions[bot]" -c user.email="41898282+github-actions[bot]@users.noreply.github.com" commit --quiet -m "Update to $VERSION"
git push --quiet origin HEAD:master
echo "✓ Pushed sikemux-bin $VERSION to the AUR"
