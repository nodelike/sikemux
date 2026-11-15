#!/bin/sh
# Points the system's package manager at Sikemux's repository for this
# package's channel, so the package updates with the system. A package of the
# other channel rewrites it, which is how the installer switches channel.
RELEASES=https://github.com/nodelike/sikemux/releases/download
SUITE=@SUITE@

if [ -f /usr/share/keyrings/sikemux-archive.asc ] && [ -d /etc/apt/sources.list.d ]; then
  cat >/etc/apt/sources.list.d/sikemux.sources <<SOURCE
Types: deb
URIs: $RELEASES
Suites: $SUITE/
Signed-By: /usr/share/keyrings/sikemux-archive.asc
SOURCE
fi
# zypper fetches each .rpm beside the index rather than from the release it names,
# so openSUSE keeps updating through the installer.
if [ -f /etc/pki/rpm-gpg/RPM-GPG-KEY-sikemux ] && [ -d /etc/yum.repos.d ] && command -v dnf >/dev/null 2>&1; then
  cat >/etc/yum.repos.d/sikemux.repo <<REPO
[sikemux]
name=Sikemux
baseurl=$PAGES/$SUITE/
enabled=1
gpgcheck=1
repo_gpgcheck=1
gpgkey=file:///etc/pki/rpm-gpg/RPM-GPG-KEY-sikemux
REPO
fi
