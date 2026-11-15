#!/bin/sh
# Installs 1.0.0 from its .deb, as the installer does, then upgrades it through
# the repository while the suite moves to 1.0.1 step by step as
# scripts/move-package-suite.sh moves it.
set -eu
S=/serve/objects/packages-stable
N=/proof/index-1.0.1/apt/stable/gz
installed() { dpkg-query -W -f '${Version}' sikemux; }
apt-get install -y -qq /serve/objects/v1.0.0/Sikemux_1.0.0_amd64.deb >/dev/null
cat /etc/apt/sources.list.d/sikemux.sources
update() {
  apt-get update 2>&1 | tee /tmp/update
  if grep -q '^[WE]:' /tmp/update; then exit 1; fi
}
candidate() { apt-cache policy sikemux | sed -n 's/ *Candidate: //p'; }
echo "== suite on 1.0.0"
update
apt-cache policy sikemux | tee /tmp/policy; grep -q 'releases/download packages-stable/' /tmp/policy

echo "== the new index is up, InRelease still names the old one"
cp $N/Packages.gz $S/
update; [ "$(candidate)" = 1.0.0 ]

echo "== InRelease is gone for a moment: apt reads Release and Release.gpg"
mv $S/InRelease $S/InRelease.old
update; [ "$(candidate)" = 1.0.0 ]

echo "== InRelease names the new index; Release is still the old one"
cp $N/InRelease $S/InRelease
update; [ "$(candidate)" = 1.0.1 ]
cp $N/Release $N/Release.gpg $S/
rm $S/Packages.xz $S/InRelease.old

echo "== upgrade"
apt-get install -y -qq --only-upgrade sikemux >/dev/null
[ "$(installed)" = 1.0.1 ] && [ "$(/usr/lib/sikemux/sikemux-host)" = 1.0.1 ]
grep -q 'Suites: packages-stable/' /etc/apt/sources.list.d/sikemux.sources
[ "$(stat -c %G /usr/lib/sikemux/sikemux-host)" = sikemux ]

echo "== remove"
apt-get remove -y -qq sikemux >/dev/null
[ ! -e /etc/apt/sources.list.d/sikemux.sources ]
apt-get update -qq
echo "✓ apt upgraded 1.0.0 to 1.0.1 through the repository"
