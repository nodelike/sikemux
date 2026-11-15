#!/bin/sh
# Installs the previous nightly's .deb from its release, then upgrades it to the
# new nightly through the real packages-nightly suite on GitHub.
set -eu
OLD="$1"
NEW="$2"
DEB_NEW=$(echo "$NEW" | sed 's/-/~/')
REL=https://github.com/nodelike/sikemux/releases/download
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq && apt-get install -y -qq curl ca-certificates >/dev/null
curl -fsSLo /tmp/old.deb "$REL/v$OLD/Sikemux_${OLD}_amd64.deb"
apt-get install -y -qq /tmp/old.deb >/dev/null
echo "== installed $(dpkg-query -W -f '${Version}' sikemux)"
if [ ! -f /etc/apt/sources.list.d/sikemux.sources ]; then
  echo "== $OLD writes no source; adding it as the install docs do"
  curl -fsSLo /usr/share/keyrings/sikemux-archive.asc \
    https://raw.githubusercontent.com/nodelike/sikemux/main/packaging/linux/sikemux-archive.asc
  printf 'Types: deb\nURIs: %s\nSuites: packages-nightly/\nSigned-By: /usr/share/keyrings/sikemux-archive.asc\n' "$REL" \
    >/etc/apt/sources.list.d/sikemux.sources
fi
cat /etc/apt/sources.list.d/sikemux.sources
apt-get update 2>&1 | tee /tmp/update
if grep -q '^[WE]:' /tmp/update; then exit 1; fi
apt-cache policy sikemux
[ "$(apt-cache policy sikemux | sed -n 's/ *Candidate: //p')" = "$DEB_NEW" ]
apt-get install -y -qq --only-upgrade sikemux >/dev/null
[ "$(dpkg-query -W -f '${Version}' sikemux)" = "$DEB_NEW" ]
sikemux-host --version
[ "$(stat -c %G /usr/lib/sikemux/sikemux-host)" = sikemux ]
grep -q 'Suites: packages-nightly/' /etc/apt/sources.list.d/sikemux.sources
test -f /usr/share/keyrings/sikemux-archive.asc
apt-get update 2>&1 | tee /tmp/update
if grep -q '^[WE]:' /tmp/update; then exit 1; fi
echo "✓ apt upgraded $OLD to $NEW through packages-nightly"
