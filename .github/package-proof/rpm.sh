#!/bin/sh
# Installs 1.0.0 from its .rpm, as the installer does, then upgrades it through
# the repository once Pages serves the 1.0.1 index. dnf or zypper, whichever is there.
set -eu
P=/serve/pages/packages-stable
RPM=/serve/objects/v1.0.0/Sikemux-1.0.0-1.x86_64.rpm
if command -v dnf >/dev/null; then
  dnf install -y -q "$RPM"
  cat /etc/yum.repos.d/sikemux.repo
  dnf makecache -y -q --repo sikemux
  [ "$(dnf repoquery -q --repo sikemux --qf '%{version}' sikemux | grep . | tail -n 1)" = 1.0.0 ]
  rm -rf $P/repodata && cp -R /proof/index-1.0.1/rpm/repodata $P/repodata
  dnf upgrade -y -q --refresh sikemux
  [ "$(rpm -q --qf '%{version}' sikemux)" = 1.0.1 ] && [ "$(/usr/lib/sikemux/sikemux-host)" = 1.0.1 ]
  dnf remove -y -q sikemux
  [ ! -e /etc/yum.repos.d/sikemux.repo ]
else
  zypper --non-interactive --no-gpg-checks install --allow-unsigned-rpm "$RPM"
  [ "$(/usr/lib/sikemux/sikemux-host)" = 1.0.0 ]
  [ ! -e /etc/zypp/repos.d/sikemux.repo ] && [ ! -e /etc/yum.repos.d/sikemux.repo ]
  zypper --non-interactive remove sikemux
  echo "✓ zypper installs the .rpm and gets no repository"
  exit 0
fi
echo "✓ the repository upgraded 1.0.0 to 1.0.1"
