#!/bin/sh
# Builds sikemux-device.jar, the dex Sikemux runs on Android devices with app_process.
# Needs a JDK and an Android SDK with a platform and build-tools.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
sdk=${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}}
platform=$(ls -d "$sdk"/platforms/android-* | sort -V | tail -1)
tools=$(ls -d "$sdk"/build-tools/* | sort -V | tail -1)
classes=$(mktemp -d)
trap 'rm -rf "$classes"' EXIT
javac --release 11 -cp "$platform/android.jar" -d "$classes" $(find "$here/src" -name '*.java')
"$tools/d8" --release --min-api 26 --lib "$platform/android.jar" --output "$here/sikemux-device.jar" $(find "$classes" -name '*.class')
