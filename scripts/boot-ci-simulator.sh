#!/usr/bin/env bash
# Boots CI's simulator while the job builds, and stops two daemons that keep the runner's three
# cores busy long after boot: apsd retries Apple's push servers several times a second, failing
# every TLS handshake on GitHub's runners, and mediaanalysisd spends minutes on two cores
# analysing the sample photos, starting some ten minutes after boot.
set -euo pipefail

device="$1"
xcrun simctl boot "$device"
xcrun simctl bootstatus "$device" -b >/dev/null
for daemon in com.apple.apsd com.apple.mediaanalysisd; do
  xcrun simctl spawn "$device" launchctl bootout "system/$daemon"
done
# The tests open Settings first, and its first launch is the slow one.
xcrun simctl launch "$device" com.apple.Preferences
sleep 20
xcrun simctl terminate "$device" com.apple.Preferences
echo "The simulator is ready"
