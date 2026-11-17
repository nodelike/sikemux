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
