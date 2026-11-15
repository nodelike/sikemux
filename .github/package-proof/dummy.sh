#!/usr/bin/env bash
# Packs a small .deb and .rpm named sikemux with the real install and removal
# scripts, laid out as scripts/build-linux.sh lays them out.
#   dummy.sh <version> <bundle dir>
set -euo pipefail
VERSION="$1"
BUNDLE="$2"
ROOT="$PWD"
PACKAGE_VERSION="${VERSION/-/\~}"
SUITE=packages-stable
[[ "$VERSION" == *-nightly.* ]] && SUITE=packages-nightly
after_install() {
  cat "$ROOT/packaging/linux/postinst.sh"
  sed -e '1{/^#!/d}' -e "s|@SUITE@|$SUITE|" "$ROOT/packaging/linux/repository.sh"
}
WORK="$(mktemp -d)"
mkdir -p "$WORK/package/usr/lib/sikemux" "$BUNDLE/deb" "$BUNDLE/rpm"
printf '#!/bin/sh\necho %s\n' "$VERSION" >"$WORK/package/usr/lib/sikemux/sikemux-host"
chmod 755 "$WORK/package/usr/lib/sikemux/sikemux-host"

DEB="$WORK/deb"
cp -a "$WORK/package" "$DEB"
mkdir -p "$DEB/DEBIAN"
install -D -m 644 packaging/linux/sikemux-archive.asc "$DEB/usr/share/keyrings/sikemux-archive.asc"
cat >"$DEB/DEBIAN/control" <<CONTROL
Package: sikemux
Version: $PACKAGE_VERSION
Architecture: amd64
Maintainer: nodelike
Description: proof
CONTROL
after_install >"$DEB/DEBIAN/postinst"
install -m 755 packaging/linux/postrm.sh "$DEB/DEBIAN/postrm"
chmod 755 "$DEB/DEBIAN/postinst"
dpkg-deb --root-owner-group -Zxz --build "$DEB" "$BUNDLE/deb/Sikemux_${VERSION}_amd64.deb" >/dev/null

mkdir -p "$WORK/rpm/SPECS"
cat >"$WORK/rpm/SPECS/sikemux.spec" <<SPEC
Name: sikemux
Version: $PACKAGE_VERSION
Release: 1
Summary: proof
License: MIT
BuildArch: x86_64
%global debug_package %{nil}
%description
proof
%install
cp -a "$WORK/package"/. %{buildroot}/
install -D -m 644 "$ROOT/packaging/linux/sikemux-archive.asc" %{buildroot}/etc/pki/rpm-gpg/RPM-GPG-KEY-sikemux
%post
$(after_install | sed '1{/^#!/d}')
%postun
$(sed '1{/^#!/d}' "$ROOT/packaging/linux/postrm.sh")
%files
/usr/lib/sikemux
/etc/pki/rpm-gpg/RPM-GPG-KEY-sikemux
SPEC
rpmbuild --quiet --define "_topdir $WORK/rpm" -bb "$WORK/rpm/SPECS/sikemux.spec"
mv "$WORK/rpm/RPMS/x86_64/"*.rpm "$BUNDLE/rpm/Sikemux-${VERSION}-1.x86_64.rpm"
