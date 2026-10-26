#!/usr/bin/env bash
# Build the x86_64 Windows app as a zip and a setup.exe: the GPUI app with its core beside it, unsigned.
# Run from Git Bash with the Visual Studio C++ build tools installed.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd -P)"
VERSION="$(node -p "require('./package.json').version")"

fail() {
  echo "Windows packaging failed: $*" >&2
  exit 1
}

[[ "$(uname -s)" == MINGW* || "$(uname -s)" == MSYS* ]] || fail "run this from Git Bash on Windows"
for command in cargo node powershell iexpress; do
  command -v "$command" >/dev/null || fail "missing required command: $command"
done

(cd src-tauri && cargo build --release --locked -p sikemux-host)
(cd src-tauri/crates/sikemux-gpui && cargo build --release --locked --bin sikemux)

WORK="$ROOT/src-tauri/target/release/windows"
BUNDLE="$ROOT/src-tauri/target/release/bundle/windows"
rm -rf "$WORK" "$BUNDLE"
mkdir -p "$WORK/Sikemux" "$BUNDLE"
cp "$ROOT/src-tauri/crates/sikemux-gpui/target/release/sikemux.exe" "$WORK/Sikemux/sikemux.exe"
cp "$ROOT/src-tauri/target/release/sikemux-host.exe" "$WORK/Sikemux/sikemux-host.exe"
cp "$ROOT/src-tauri/resources/sikemux_pi_tools.ts" "$WORK/Sikemux/sikemux_pi_tools.ts"

ZIP="$BUNDLE/Sikemux_${VERSION}_x64-windows.zip"
powershell -NoProfile -Command "Compress-Archive -Path '$(cygpath -w "$WORK/Sikemux")' -DestinationPath '$(cygpath -w "$ZIP")'"

SETUP_SOURCE="$WORK/setup"
mkdir -p "$SETUP_SOURCE"
cp "$WORK/Sikemux/"* "$SETUP_SOURCE/"
cp "$ROOT/scripts/windows-installer/install.ps1" "$ROOT/scripts/windows-installer/uninstall.ps1" "$SETUP_SOURCE/"
printf '%s' "$VERSION" > "$SETUP_SOURCE/version.txt"

SETUP="$BUNDLE/Sikemux_${VERSION}_x64-setup.exe"
FILES=(sikemux.exe sikemux-host.exe sikemux_pi_tools.ts install.ps1 uninstall.ps1 version.txt)
{
  printf '[Version]
Class=IEXPRESS
SEDVersion=3
[Options]
'
  printf 'PackagePurpose=InstallApp
ShowInstallProgramWindow=0
HideExtractAnimation=1
UseLongFileName=1
'
  printf 'InsideCompressed=0
CAB_FixedSize=0
CAB_ResvCodeSigning=0
RebootMode=N
'
  printf 'InstallPrompt=
DisplayLicense=
FinishMessage=
'
  printf 'TargetName=%s
FriendlyName=Sikemux
' "$(cygpath -w "$SETUP")"
  printf 'AppLaunched=powershell.exe -NoProfile -ExecutionPolicy Bypass -File install.ps1
PostInstallCmd=<None>
'
  printf 'AdminQuietInstCmd=
UserQuietInstCmd=
SourceFiles=SourceFiles
[Strings]
'
  for index in "${!FILES[@]}"; do printf 'FILE%s=%s
' "$index" "${FILES[$index]}"; done
  printf '[SourceFiles]
SourceFiles0=%s
[SourceFiles0]
' "$(cygpath -w "$SETUP_SOURCE")"
  for index in "${!FILES[@]}"; do printf '%%FILE%s%%=
' "$index"; done
} > "$WORK/setup.sed"
iexpress //N //Q "$(cygpath -w "$WORK/setup.sed")"
[[ -f "$SETUP" ]] || fail "iexpress did not write $SETUP"
echo "$SETUP"
echo "$ZIP"
