#!/usr/bin/env bash
# Moves a release binary's symbols into a debug file of their own for Sentry, which finds it
# by the binary's build ID, and strips the binary that ships.
#   scripts/keep-linux-symbols.sh <binary> <debug file>
set -euo pipefail
BINARY="${1:?usage: $0 <binary> <debug file>}"
DEBUG="${2:?usage: $0 <binary> <debug file>}"
mkdir -p "$(dirname "$DEBUG")"
objcopy --only-keep-debug "$BINARY" "$DEBUG"
strip "$BINARY"
