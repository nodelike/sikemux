#!/usr/bin/env bash
# Write the Arch package's recipe for one release into <dir>: the PKGBUILD pinned to
# that release's tarball, its .SRCINFO, the files it installs, and the tarball, so
# makepkg there builds without downloading. A nightly becomes sikemux-nightly-bin.
set -euo pipefail

VERSION="${1:-}"
TARBALL="${2:-}"
OUT="${3:-}"
PKGREL="${4:-1}"
HERE="$(cd "$(dirname "$0")" && pwd -P)"

fail() {
  echo "Arch recipe failed: $*" >&2
  exit 1
}

[[ -n "$VERSION" && -n "$TARBALL" && -n "$OUT" ]] || { echo "Usage: $0 <version> <tarball> <dir> [pkgrel]" >&2; exit 2; }
case "$VERSION" in
  *-nightly.*) NAME=sikemux-nightly-bin ;;
  *-*) fail "$VERSION is neither a nightly nor a stable version" ;;
  *) NAME=sikemux-bin ;;
esac
[[ "$(basename "$TARBALL")" == "Sikemux_${VERSION}_amd64.tar.gz" ]] || fail "$TARBALL is not the tarball for $VERSION"
[[ -s "$TARBALL" ]] || fail "missing $TARBALL"

sum() { sha256sum "$1" | cut -d ' ' -f 1; }

mkdir -p "$OUT"
install -m 644 "$HERE/sikemux.sysusers" "$HERE/sikemux.tmpfiles" "$OUT/"
cp "$TARBALL" "$OUT/"
sed -e "s/^pkgname=.*/pkgname=$NAME/" \
  -e "s/^_version=.*/_version=$VERSION/" \
  -e "s/^pkgrel=.*/pkgrel=$PKGREL/" \
  -e "s/^sha256sums=.*/sha256sums=('$(sum "$TARBALL")' '$(sum "$HERE/sikemux.sysusers")' '$(sum "$HERE/sikemux.tmpfiles")')/" \
  "$HERE/PKGBUILD" >"$OUT/PKGBUILD"
grep -qx "_version=$VERSION" "$OUT/PKGBUILD" || fail "the PKGBUILD has no _version line to set"
(cd "$OUT" && makepkg --printsrcinfo >.SRCINFO)
grep -q "sha256sums = $(sum "$TARBALL")" "$OUT/.SRCINFO" || fail "the PKGBUILD does not pin the tarball"
echo "✓ $NAME $(sed -n 's/^\tpkgver = //p' "$OUT/.SRCINFO")-$PKGREL in $OUT"
