#!/usr/bin/env bash
# Builds the core's phone client (crates/sikemux-mobile) for iOS: the static
# library as an xcframework, and its Swift bindings, into mobile/ios/build.
#   build-core.sh [--profile debug|mobile] [--sim-only|--device-only]
set -euo pipefail

ios="$(cd "$(dirname "$0")/.." && pwd)"
repo="$(cd "$ios/../.." && pwd)"
export PATH="$repo/scripts/bin:$HOME/.cargo/bin:$PATH"
export IPHONEOS_DEPLOYMENT_TARGET=17.0

profile=debug
targets=(aarch64-apple-ios-sim aarch64-apple-ios)
while [ $# -gt 0 ]; do
  case "$1" in
    --profile) profile="$2"; shift 2 ;;
    --sim-only) targets=(aarch64-apple-ios-sim); shift ;;
    --device-only) targets=(aarch64-apple-ios); shift ;;
    *) echo "build-core.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done

cd "$repo"
target_dir="$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
profile_dir="$profile"
[ "$profile" = dev ] && profile_dir=debug
[ "$profile" = debug ] && profile=dev

for target in "${targets[@]}"; do
  cargo build -p sikemux-mobile --lib --target "$target" --profile "$profile"
done

out="$ios/build"
bindings="$out/bindings"
headers="$out/headers"
rm -rf "$bindings" "$headers" "$out/SikemuxCore.xcframework"
mkdir -p "$bindings" "$headers"

library() { echo "$target_dir/$1/$profile_dir/libsikemux_mobile.a"; }

cargo run -q -p sikemux-mobile --features bindgen --bin uniffi-bindgen -- \
  generate --library "$(library "${targets[0]}")" --language swift --out-dir "$bindings"

cp "$bindings"/*FFI.h "$headers/"
cat "$bindings"/*FFI.modulemap > "$headers/module.modulemap"

args=()
for target in "${targets[@]}"; do
  args+=(-library "$(library "$target")" -headers "$headers")
done
xcodebuild -create-xcframework "${args[@]}" -output "$out/SikemuxCore.xcframework" >/dev/null
echo "Built $out/SikemuxCore.xcframework ($profile_dir) and $bindings"
