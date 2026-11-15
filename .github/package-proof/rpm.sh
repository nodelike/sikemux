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
  rm -rf $P/repodata && cp -R /proof/index-1.0.1/rpm/repodata $P/repodata && cp /serve/objects/v1.0.1/*.rpm $P/
  dnf upgrade -y -q --refresh sikemux
  [ "$(rpm -q --qf '%{version}' sikemux)" = 1.0.1 ] && [ "$(/usr/lib/sikemux/sikemux-host)" = 1.0.1 ]
  dnf remove -y -q sikemux
  [ ! -e /etc/yum.repos.d/sikemux.repo ]
else
  rpm --import /proof/archive.asc
  zypper --non-interactive install "$RPM"
  cat /etc/zypp/repos.d/sikemux.repo
  zypper --non-interactive --gpg-auto-import-keys refresh sikemux
  rm -rf $P/repodata && cp -R /proof/index-1.0.1/rpm/repodata $P/repodata && cp /serve/objects/v1.0.1/*.rpm $P/
  zypper --non-interactive --gpg-auto-import-keys refresh sikemux
  zypper --non-interactive update sikemux
  [ "$(rpm -q --qf '%{version}' sikemux)" = 1.0.1 ] && [ "$(/usr/lib/sikemux/sikemux-host)" = 1.0.1 ]
  zypper --non-interactive remove sikemux
  [ ! -e /etc/zypp/repos.d/sikemux.repo ]
fi
echo "✓ the repository upgraded 1.0.0 to 1.0.1"
