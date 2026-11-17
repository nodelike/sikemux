#!/usr/bin/env bash
# shellcheck disable=SC2154 # GitHub sets the runner's variables.
# Installs a pinned XcodeGen on a CI runner and puts it on the job's PATH.
# Homebrew takes minutes to update itself first; this takes seconds.
set -euo pipefail

version=2.46.0
sha256=4d9e34b62172d645eed6457cac13fc222569974098ef4ee9c3368bedf0196806
zip="$RUNNER_TEMP/xcodegen.zip"

curl -fsSL --retry 3 -o "$zip" "https://github.com/yonaskolb/XcodeGen/releases/download/$version/xcodegen.zip"
echo "$sha256  $zip" | shasum -a 256 -c -
unzip -q -o "$zip" -d "$RUNNER_TEMP"
echo "$RUNNER_TEMP/xcodegen/bin" >>"$GITHUB_PATH"
