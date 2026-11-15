#!/usr/bin/env bash
# Makes a stable candidate the latest release, which is what the stable feed, the installer and the
# site's download follow, then moves both package suites to it. Only the Ship workflow runs it.
#   ship-release.sh <version>
set -euo pipefail

cd "$(dirname "$0")/.."
VERSION="${1:?usage: $0 <version>}"
[[ "${GITHUB_ACTIONS:-}" == "true" ]] || { echo "releases ship from the Ship workflow" >&2; exit 1; }

echo "→ Shipping v$VERSION to stable"
gh release edit "v$VERSION" --prerelease=false --latest
gh workflow run packages.yml -f suite=stable
gh workflow run packages.yml -f suite=nightly
echo "✓ v$VERSION is the stable release"
