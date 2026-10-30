#!/usr/bin/env bash
# Build the x86_64 Linux desktop packages: the GPUI app with its core beside it, as
# a deb, an rpm and an AppImage, named as every release has named them, since the
# installer downloads them by name. They carry no updater signature.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd -P)"
VERSION="$(node -p "require('./package.json').version")"
# apt and dnf rank 0.6.0-nightly.1 above 0.6.0 and 0.6.0~nightly.1 below it, so the
# packages say ~ to let a stable release overtake its nightlies.
PACKAGE_VERSION="${VERSION/-/\~}"
SUMMARY="$(node scripts/app-metadata.mjs description)"
TRIPLE=x86_64-unknown-linux-gnu

LINUXDEPLOY_URL=https://github.com/linuxdeploy/linuxdeploy/releases/download/1-alpha-20251107-1/linuxdeploy-x86_64.AppImage
LINUXDEPLOY_SHA256=c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d
GTK_PLUGIN_URL=https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/7a3fbc31a9e5075073ff8790f26effbac5f84453/linuxdeploy-plugin-gtk.sh
GTK_PLUGIN_SHA256=b0f4cbc684a0103a9651f0955b635eaea0096b3a66c0f5a2c2aa337960375171
GSTREAMER_PLUGIN_URL=https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gstreamer/432af639725bc8b99df12351e86bb2e8a6b3585f/linuxdeploy-plugin-gstreamer.sh
GSTREAMER_PLUGIN_SHA256=d1cf90701047aa9583c274bb66dd82db352255374fbfb8f3c4c89a05fe23bce0
APPIMAGETOOL_URL=https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage
APPIMAGETOOL_SHA256=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
# The program at the start of every AppImage, which appimagetool would otherwise download unpinned.
RUNTIME_URL=https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64
RUNTIME_SHA256=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d

# Libraries the app opens while it runs rather than links, which no dependency scan finds.
DEB_OPENED="libvulkan1, libegl1, libwayland-client0, libx11-6, libxext6"
RPM_OPENED=(libvulkan.so.1 libEGL.so.1 libwayland-client.so.0 libX11.so.6 libXext.so.6)
# GStreamer plugins browser recording loads by name: appsrc, videoconvert, the queue and caps
# elements, VP8 in WebM, and MP4. Packages ask for the sets holding them, and suggest the one
# with an H.264 encoder, without which a recording is WebM. The AppImage carries the whole of
# the base and good sets with the core elements, as the pages in desk tabs play video and sound
# through them too.
DEB_PLUGINS="gstreamer1.0-plugins-base, gstreamer1.0-plugins-good"
DEB_H264=gstreamer1.0-plugins-ugly
RPM_PLUGINS=("(gstreamer1-plugins-base or gstreamer-plugins-base)" "(gstreamer1-plugins-good or gstreamer-plugins-good)")
RPM_H264="(gstreamer1-plugins-ugly or gstreamer-plugins-ugly)"
RECORDING_PLUGINS=(coreelements app videoconvertscale vpx matroska isomp4)
GSTREAMER_SETS=(gstreamer1.0-plugins-base gstreamer1.0-plugins-good)
GSTREAMER_DIR=/usr/lib/x86_64-linux-gnu/gstreamer-1.0

fail() {
  echo "Linux packaging failed: $*" >&2
  exit 1
}

[[ "$(uname -s)-$(uname -m)" == "Linux-x86_64" ]] || fail "Linux x86_64 is required"
[[ -n "$SUMMARY" ]] || fail "the app has no description in app/Cargo.toml"
for command in cargo node dpkg-deb dpkg-shlibdeps rpmbuild patchelf curl sha256sum; do
  command -v "$command" >/dev/null || fail "missing required command: $command"
done

BUNDLE="$ROOT/target/release/bundle"
WORK="$ROOT/target/release/linux"
TOOLS="$WORK/tools"
rm -rf "$WORK" "$BUNDLE/deb" "$BUNDLE/rpm" "$BUNDLE/appimage"
mkdir -p "$TOOLS" "$BUNDLE/deb" "$BUNDLE/rpm" "$BUNDLE/appimage"

fetch() {
  local url="$1" sha="$2" file="$TOOLS/$3"
  curl -fsSL --retry 3 -o "$file" "$url"
  echo "$sha  $file" | sha256sum -c --quiet - || fail "$3 does not match its pinned checksum"
  chmod +x "$file"
}

echo "→ Building the core"
node scripts/build-cli-sidecar.mjs
CORE="$ROOT/packaging/binaries/sikemux-host-$TRIPLE"
echo "→ Building the app"
(cd app && cargo build --release --locked --bin sikemux)
APP="$ROOT/target/release/sikemux"
PI_TOOLS="$ROOT/crates/sikemux-core/resources/sikemux_pi_tools.ts"
POSTINST="$ROOT/packaging/linux/postinst.sh"

desktop_entry() {
  cat <<DESKTOP
[Desktop Entry]
Type=Application
Name=Sikemux
Comment=$SUMMARY
Exec=$1 %u
Icon=sikemux
Terminal=false
Categories=Development;
StartupWMClass=com.nodelike.sikemux
MimeType=x-scheme-handler/sikemux;
DESKTOP
}

lay_out_shared() {
  local root="$1"
  mkdir -p "$root/usr/share/applications"
  desktop_entry sikemux >"$root/usr/share/applications/com.nodelike.sikemux.desktop"
  for icon in 32x32:32x32 128x128:128x128 128x128@2x:256x256 icon:512x512; do
    mkdir -p "$root/usr/share/icons/hicolor/${icon#*:}/apps"
    install -m 644 "$ROOT/packaging/icons/${icon%%:*}.png" "$root/usr/share/icons/hicolor/${icon#*:}/apps/sikemux.png"
  done
}

# The core finds its Pi extension, and the app its core, in the folder their own
# program is in, so all three sit in one.
PACKAGE="$WORK/package"
mkdir -p "$PACKAGE/usr/lib/sikemux" "$PACKAGE/usr/bin"
install -m 755 "$APP" "$PACKAGE/usr/lib/sikemux/sikemux"
install -m 755 "$CORE" "$PACKAGE/usr/lib/sikemux/sikemux-host"
install -m 644 "$PI_TOOLS" "$PACKAGE/usr/lib/sikemux/sikemux_pi_tools.ts"
ln -s ../lib/sikemux/sikemux "$PACKAGE/usr/bin/sikemux"
ln -s ../lib/sikemux/sikemux-host "$PACKAGE/usr/bin/sikemux-host"
lay_out_shared "$PACKAGE"

echo "→ Packing the deb"
DEB_ROOT="$WORK/deb"
cp -a "$PACKAGE" "$DEB_ROOT"
mkdir -p "$DEB_ROOT/DEBIAN" "$WORK/shlibs/debian"
touch "$WORK/shlibs/debian/control"
LINKED="$(cd "$WORK/shlibs" && dpkg-shlibdeps -O "$DEB_ROOT/usr/lib/sikemux/sikemux" "$DEB_ROOT/usr/lib/sikemux/sikemux-host" | sed -n 's/^shlibs:Depends=//p')"
[[ -n "$LINKED" ]] || fail "dpkg-shlibdeps found no libraries"
# Ubuntu 24.04 renamed libraries such as libgtk-3-0 to libgtk-3-0t64 and still answers
# to the old names, which are the only ones older Debian and Ubuntu know.
LINKED="$(sed -E 's/([a-z0-9.+-]+)t64( |,|$)/\1\2/g' <<<"$LINKED")"
cat >"$DEB_ROOT/DEBIAN/control" <<CONTROL
Package: sikemux
Version: $PACKAGE_VERSION
Architecture: amd64
Maintainer: nodelike
Section: devel
Priority: optional
Installed-Size: $(du -sk --exclude=DEBIAN "$DEB_ROOT" | cut -f1)
Depends: $LINKED, $DEB_OPENED, $DEB_PLUGINS
Recommends: $DEB_H264
Description: $SUMMARY
CONTROL
install -m 755 "$POSTINST" "$DEB_ROOT/DEBIAN/postinst"
dpkg-deb --root-owner-group -Zxz --build "$DEB_ROOT" "$BUNDLE/deb/Sikemux_${VERSION}_amd64.deb" >/dev/null

echo "→ Packing the rpm"
RPM_TOP="$WORK/rpm"
mkdir -p "$RPM_TOP/SPECS"
{
  cat <<SPEC
Name: sikemux
Version: $PACKAGE_VERSION
Release: 1
Summary: $SUMMARY
License: FSL-1.1-MIT
BuildArch: x86_64
AutoReqProv: yes
SPEC
  for library in "${RPM_OPENED[@]}"; do echo "Requires: $library()(64bit)"; done
  for plugins in "${RPM_PLUGINS[@]}"; do echo "Requires: $plugins"; done
  echo "Recommends: $RPM_H264"
  cat <<SPEC
%global debug_package %{nil}
%global __strip /bin/true
%global _build_id_links none

%description
$SUMMARY

%install
cp -a "$PACKAGE"/. %{buildroot}/

%post
$(sed '1{/^#!/d}' "$POSTINST")

%files
/usr/lib/sikemux
/usr/bin/sikemux
/usr/bin/sikemux-host
/usr/share/applications/com.nodelike.sikemux.desktop
/usr/share/icons/hicolor/*/apps/sikemux.png
SPEC
} >"$RPM_TOP/SPECS/sikemux.spec"
rpmbuild --quiet --define "_topdir $RPM_TOP" -bb "$RPM_TOP/SPECS/sikemux.spec"
RPMS=("$RPM_TOP"/RPMS/x86_64/*.rpm)
[[ ${#RPMS[@]} -eq 1 ]] || fail "rpmbuild made ${#RPMS[@]} packages"
mv "${RPMS[0]}" "$BUNDLE/rpm/Sikemux-${VERSION}-1.x86_64.rpm"

echo "→ Packing the AppImage"
# The tools are AppImages themselves, and a build machine may have no FUSE to mount them.
export APPIMAGE_EXTRACT_AND_RUN=1
fetch "$LINUXDEPLOY_URL" "$LINUXDEPLOY_SHA256" linuxdeploy
fetch "$GTK_PLUGIN_URL" "$GTK_PLUGIN_SHA256" linuxdeploy-plugin-gtk.sh
fetch "$GSTREAMER_PLUGIN_URL" "$GSTREAMER_PLUGIN_SHA256" linuxdeploy-plugin-gstreamer.sh
fetch "$APPIMAGETOOL_URL" "$APPIMAGETOOL_SHA256" appimagetool
fetch "$RUNTIME_URL" "$RUNTIME_SHA256" runtime
APPDIR="$WORK/Sikemux.AppDir"
mkdir -p "$APPDIR/usr/bin"
install -m 755 "$APP" "$APPDIR/usr/bin/sikemux"
install -m 755 "$CORE" "$APPDIR/usr/bin/sikemux-host"
install -m 644 "$PI_TOOLS" "$APPDIR/usr/bin/sikemux_pi_tools.ts"
lay_out_shared "$APPDIR"
# WebKitGTK starts its page and network processes from the folder it was built for, which
# no library scan follows, so they go in at the same place inside the AppImage.
WEBKIT_DIR="$(pkg-config --variable=libdir webkit2gtk-4.1)/webkit2gtk-4.1"
[[ -d "$WEBKIT_DIR" ]] || fail "WebKitGTK's processes are not at $WEBKIT_DIR"
mkdir -p "$APPDIR$WEBKIT_DIR"
for part in WebKitWebProcess WebKitNetworkProcess WebKitGPUProcess injected-bundle; do
  [[ -e "$WEBKIT_DIR/$part" ]] && cp -a "$WEBKIT_DIR/$part" "$APPDIR$WEBKIT_DIR/"
done
mkdir -p "$WORK/gstreamer"
cp "$GSTREAMER_DIR/libgstcoreelements.so" "$WORK/gstreamer/"
dpkg -L "${GSTREAMER_SETS[@]}" | grep -x "$GSTREAMER_DIR/libgst[^/]*\.so" | xargs -I{} cp {} "$WORK/gstreamer/"
for plugin in "${RECORDING_PLUGINS[@]}"; do
  [[ -f "$WORK/gstreamer/libgst$plugin.so" ]] || fail "GStreamer's $plugin plugin, which recording needs, is not installed"
done
(
  cd "$TOOLS"
  export DEPLOY_GTK_VERSION=3 NO_STRIP=1 GSTREAMER_PLUGINS_DIR="$WORK/gstreamer"
  ./linuxdeploy --appdir "$APPDIR" \
    --executable "$APPDIR/usr/bin/sikemux" --executable "$APPDIR/usr/bin/sikemux-host" \
    --deploy-deps-only "$APPDIR$WEBKIT_DIR" \
    --desktop-file "$APPDIR/usr/share/applications/com.nodelike.sikemux.desktop" \
    --icon-file "$APPDIR/usr/share/icons/hicolor/512x512/apps/sikemux.png" \
    --plugin gtk --plugin gstreamer
)
# The same length keeps the libraries intact, and turns the processes' /usr/lib/… into
# lib/…, found from usr/ inside the AppImage, which AppRun starts in.
find "$APPDIR"/usr/lib* -name 'libwebkit*' -type f -exec sed -i -e 's|/usr|././|g' {} +
rm -f "$APPDIR/AppRun" "$APPDIR/AppRun.wrapped"
cat >"$APPDIR/AppRun" <<'APPRUN'
#!/bin/sh
APPDIR="$(dirname "$(readlink -f "$0")")"
export APPDIR
for hook in "$APPDIR"/apprun-hooks/*.sh; do
  [ -f "$hook" ] && . "$hook"
done
cd "$APPDIR/usr" || exit 1
exec "$APPDIR/usr/bin/sikemux" "$@"
APPRUN
chmod 755 "$APPDIR/AppRun"
(
  cd "$TOOLS"
  export ARCH=x86_64 VERSION
  ./appimagetool --no-appstream --runtime-file "$TOOLS/runtime" "$APPDIR" "$BUNDLE/appimage/Sikemux_${VERSION}_amd64.AppImage"
)

for package in "$BUNDLE/deb/Sikemux_${VERSION}_amd64.deb" "$BUNDLE/rpm/Sikemux-${VERSION}-1.x86_64.rpm" "$BUNDLE/appimage/Sikemux_${VERSION}_amd64.AppImage"; do
  [[ -s "$package" ]] || fail "missing $package"
  echo "✓ $(basename "$package") ($(du -h "$package" | cut -f1))"
done
