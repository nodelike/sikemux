#!/usr/bin/env bash
# Gives the workspace the version passed in place of the one in package.json.
# Usage: set-version.sh <version>
set -euo pipefail

cd "$(dirname "$0")/.."
version="$1"
from="$(node -p 'require("./package.json").version')"

sed -i.bak "s/\"version\": \"$from\"/\"version\": \"$version\"/" package.json
sed -i.bak "s/^version = \"$from\"$/version = \"$version\"/" Cargo.toml
# Crates from a registry or Git may share the version too; only the workspace's own have no source.
awk -v from="version = \"$from\"" -v to="version = \"$version\"" '
  held != "" { print ($0 ~ /^source = / ? held : to); held = "" }
  $0 == from { held = $0; next }
  { print }
  END { if (held != "") print to }
' Cargo.lock >Cargo.lock.bak && mv Cargo.lock.bak Cargo.lock
rm -f package.json.bak Cargo.toml.bak
