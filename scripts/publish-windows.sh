#!/usr/bin/env bash
# Adds a published nightly's signed Windows installers to its release and its latest.json, then
# repoints any feed already serving that nightly, so the Mac and Linux builds never wait for Windows.
#   publish-windows.sh <version>
set -euo pipefail

cd "$(dirname "$0")/.."
VERSION="${1:-}"
[[ "$VERSION" == *-nightly.* ]] || { echo "Usage: $0 <nightly version>" >&2; exit 2; }
[[ "${GITHUB_ACTIONS:-}" == "true" ]] || { echo "releases publish from the Release workflow" >&2; exit 1; }

shopt -s nullglob
INSTALLERS=(target/windows/Sikemux_"$VERSION"_*-setup.exe)
shopt -u nullglob
[[ ${#INSTALLERS[@]} -eq 2 ]] || { echo "expected the x64 and arm64 installers, found ${#INSTALLERS[@]}" >&2; exit 1; }
for installer in "${INSTALLERS[@]}"; do
  node scripts/verify-updater-signature.mjs "$installer" "$installer.sig"
done

dir="$(mktemp -d)"
trap 'rm -rf "$dir"' EXIT
gh release download "v$VERSION" -p latest.json -D "$dir"
RELEASE_URL="https://github.com/nodelike/sikemux/releases/download/v$VERSION" \
  MANIFEST="$dir/latest.json" python3 - "${INSTALLERS[@]}" <<'PY'
import json, os, pathlib, re, sys
path = pathlib.Path(os.environ["MANIFEST"])
manifest = json.loads(path.read_text())
arches = {"x64": "x86_64", "arm64": "aarch64"}
for file in map(pathlib.Path, sys.argv[1:]):
    arch = arches[re.search(r"_(x64|arm64)-setup\.exe$", file.name).group(1)]
    manifest["platforms"][f"windows-{arch}"] = {
        "signature": pathlib.Path(f"{file}.sig").read_text().strip(),
        "url": f'{os.environ["RELEASE_URL"]}/{file.name}',
    }
path.write_text(json.dumps(manifest, indent=2) + "\n")
PY

gh release upload "v$VERSION" "${INSTALLERS[@]}"
gh release upload "v$VERSION" "$dir/latest.json" --clobber
echo "✓ v$VERSION carries the Windows installers"
for feed in staff nightly; do
  serving="$(gh release download "$feed" -p latest.json -O - 2>/dev/null | jq -r .version || true)"
  if [[ "$serving" == "$VERSION" ]]; then scripts/move-feed.sh "$feed" "$VERSION"; fi
done
