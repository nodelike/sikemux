#!/usr/bin/env bash
# Build a release's Arch package from its tarball and prove it installs: run as root
# in an Arch container. It lints the package, installs it, upgrades it to a rebuild,
# and after each checks the core is still setgid to its secret store's group.
# Leaves the package and the AUR's files in <dir>.
set -euo pipefail

VERSION="${1:-}"
TARBALL="${2:-}"
OUT="${3:-}"
HERE="$(cd "$(dirname "$0")" && pwd -P)"

fail() {
  echo "Arch package failed: $*" >&2
  exit 1
}

[[ -n "$VERSION" && -n "$TARBALL" && -n "$OUT" ]] || { echo "Usage: $0 <version> <tarball> <dir>" >&2; exit 2; }
[[ "$(id -u)" -eq 0 ]] || fail "run as root in a throwaway Arch container; it installs the package"
for command in makepkg namcap pacman runuser; do
  command -v "$command" >/dev/null || fail "missing required command: $command"
done

# makepkg refuses to run as root.
id builder >/dev/null 2>&1 || useradd --create-home builder
WORK="$(mktemp -d)"
chown builder "$WORK"
build() {
  local dir="$WORK/$1"
  runuser -u builder -- "$HERE/prepare.sh" "$VERSION" "$TARBALL" "$dir" "$1"
  (cd "$dir" && runuser -u builder -- makepkg --nodeps --noconfirm >/dev/null)
  local packages=("$dir"/*.pkg.tar.zst)
  [[ ${#packages[@]} -eq 1 ]] || fail "makepkg made ${#packages[@]} packages"
  BUILT="${packages[0]}"
}
build 1
FIRST="$BUILT"
build 2
UPGRADE="$BUILT"

check() {
  getent group sikemux >/dev/null || fail "$1: no group sikemux"
  [[ "$(stat -c '%a %U:%G' /usr/lib/sikemux/sikemux-host)" == "2755 root:sikemux" ]] ||
    fail "$1: the core is $(stat -c '%a %U:%G' /usr/lib/sikemux/sikemux-host), not setgid root:sikemux"
  [[ "$(stat -c '%a %U:%G' /var/lib/sikemux)" == "770 root:sikemux" ]] ||
    fail "$1: /var/lib/sikemux is $(stat -c '%a %U:%G' /var/lib/sikemux), not 770 root:sikemux"
  [[ -f /usr/share/applications/com.nodelike.sikemux.desktop ]] || fail "$1: no desktop entry"
  [[ "$(sikemux-host --version)" == "sikemux $VERSION" ]] || fail "$1: sikemux-host --version says $(sikemux-host --version)"
  local missing
  missing="$(ldd /usr/lib/sikemux/sikemux | grep 'not found' || true)"
  [[ -z "$missing" ]] || fail "$1: the package's dependencies leave the app without $missing"
  for library in libvulkan.so.1 libEGL.so.1 libwayland-client.so.0 libX11.so.6 libXext.so.6; do
    ldconfig -p | grep -q "$library" || fail "$1: no package brings $library, which the app opens"
  done
  echo "✓ $1: the core is setgid sikemux and its store is 770 root:sikemux"
}
pacman -U --noconfirm "$FIRST"
check "install"
# After installing, so namcap sees which libraries the dependencies bring.
lint="$(namcap "$WORK/1/PKGBUILD" "$FIRST" 2>&1)"
printf '%s\n' "$lint"
if grep -q ' E: ' <<<"$lint"; then fail "namcap found errors"; fi
pacman -U --noconfirm "$UPGRADE"
check "upgrade"

mkdir -p "$OUT/aur"
cp "$FIRST" "$OUT/"
cp "$WORK/1/PKGBUILD" "$WORK/1/.SRCINFO" "$WORK/1/sikemux.sysusers" "$WORK/1/sikemux.tmpfiles" "$OUT/aur/"
echo "✓ $(basename "$FIRST")"
