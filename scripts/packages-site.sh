#!/usr/bin/env bash
# Lays out what GitHub Pages serves for the rpm repository: each suite's index
# from its fixed `packages-<suite>` release, the .rpm files it names, and the key
# that signs them. dnf fetches the .rpm from the release the index points at, but
# zypper ignores that and fetches it beside the index. Pages swaps the whole site
# at once, so neither sees half a move.
#   scripts/packages-site.sh <out dir>
set -euo pipefail

cd "$(dirname "$0")/.."
OUT="${1:?usage: $0 <out dir>}"
rm -rf "$OUT"
mkdir -p "$OUT"
install -m 644 packaging/linux/sikemux-archive.asc "$OUT/sikemux-archive.asc"
served=0
for suite in stable nightly; do
  gh release view "packages-$suite" >/dev/null 2>&1 || continue
  mkdir -p "$OUT/packages-$suite"
  gh release download "packages-$suite" -p rpm-repodata.tar -O - | tar -xf - -C "$OUT/packages-$suite"
  [[ -f "$OUT/packages-$suite/repodata/repomd.xml" ]] || { echo "packages-$suite has no rpm index" >&2; exit 1; }
  version="$(cat "$OUT/packages-$suite/version")"
  rm "$OUT/packages-$suite/version"
  gh release download "v$version" -p '*.rpm' -D "$OUT/packages-$suite"
  served=$((served + 1))
done
[[ $served -gt 0 ]] || { echo "no packages-<suite> release to serve" >&2; exit 1; }
echo "✓ $OUT serves $served suites"
