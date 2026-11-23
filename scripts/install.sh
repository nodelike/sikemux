#!/bin/sh
# Installs Sikemux: curl -fsSL https://sikemux.com/install | sh
# Each release carries the installer made for its files, and sikemux.com serves
# the latest stable one; a nightly install hands over to that nightly's own.
# On Linux it asks whether to install the desktop app or just the core, which
# runs agents on a server; `| sh -s -- --host` or `--app` picks without asking,
# and `--yes` takes every default. Set SIKEMUX_CHANNEL=nightly for the nightly build, and
# SIKEMUX_SIGN_IN=later to leave signing in to whoever runs it, as Sikemux does over SSH.
set -eu

REPO="https://github.com/nodelike/sikemux"
OLDEST_GLIBC=2.34
OLDEST_APP_GLIBC=2.35
PACKAGES_KEY=31BAE8B4812C3D8B528BFB673438AB05E922705C
CHANNEL="${SIKEMUX_CHANNEL:-stable}"
SIGN_IN="${SIKEMUX_SIGN_IN:-now}"
MODE=ask
YES=false
for arg in "$@"; do
  case "$arg" in
    --host) MODE=host ;;
    --app) MODE=app ;;
    -y | --yes) YES=true ;;
    *) printf 'sikemux install: unknown option %s\n' "$arg" >&2; exit 2 ;;
  esac
done

if [ -t 1 ] && [ -z "${NO_COLOR:-}" ] && [ "${TERM:-dumb}" != dumb ]; then
  case "${COLORTERM:-}" in
    truecolor | 24bit) ACCENT=$(printf '\033[38;2;162;119;255m') ;;
    *) ACCENT=$(printf '\033[35m') ;;
  esac
  BOLD=$(printf '\033[1m')
  DIM=$(printf '\033[2m')
  RED=$(printf '\033[31m')
  RESET=$(printf '\033[0m')
  CLEAR=$(printf '\r\033[2K')
  LIVE=true
else
  ACCENT="" BOLD="" DIM="" RED="" RESET="" CLEAR="" LIVE=false
fi

say() { printf '%s\n' "$*"; }
note() { printf '  %s%s%s\n' "$DIM" "$*" "$RESET"; }
done_step() { printf '%s  %s✓%s %-32s %s%s%s\n' "$CLEAR" "$ACCENT" "$RESET" "$1" "$DIM" "${2:-}" "$RESET"; }
fail() {
  printf '%s\n  %s✗%s %s\n\n' "$CLEAR" "$RED" "$RESET" "$*" >&2
  exit 1
}

# Runs a command with a spinner beside `label`, keeping its output for when it fails.
step() {
  label=$1
  shift
  if ! $LIVE; then
    "$@" >"$TMP/step.log" 2>&1 || { sed 's/^/    /' "$TMP/step.log" >&2; fail "$label failed"; }
    return 0
  fi
  "$@" >"$TMP/step.log" 2>&1 &
  pid=$!
  i=0
  while kill -0 "$pid" 2>/dev/null; do
    case $((i % 10)) in
      0) frame='⠋' ;; 1) frame='⠙' ;; 2) frame='⠹' ;; 3) frame='⠸' ;; 4) frame='⠼' ;;
      5) frame='⠴' ;; 6) frame='⠦' ;; 7) frame='⠧' ;; 8) frame='⠇' ;; *) frame='⠏' ;;
    esac
    printf '%s  %s%s%s %s' "$CLEAR" "$ACCENT" "$frame" "$RESET" "$label"
    i=$((i + 1))
    sleep 0.08
  done
  if ! wait "$pid"; then
    printf '%s' "$CLEAR"
    sed 's/^/    /' "$TMP/step.log" >&2
    fail "$label failed"
  fi
  printf '%s' "$CLEAR"
}

# Shown to the person, never used as a path, so the ~ is meant literally.
# shellcheck disable=SC2088
home_path() {
  case "$1" in
    "$HOME"/*) printf '~/%s' "${1#"$HOME"/}" ;;
    *) printf '%s' "$1" ;;
  esac
}

command -v curl >/dev/null 2>&1 || fail "curl is required"

# The script itself arrives on stdin, so answers come from the terminal.
interactive() { ! $YES && (: </dev/tty) 2>/dev/null; }

ask() {
  printf '\n  %s›%s %s ' "$ACCENT" "$RESET" "$1" >/dev/tty
  read -r answer </dev/tty || answer=
  printf '%s' "${answer:-$2}"
}

# No screen in this session and no desktop installed: a server, even over SSH.
looks_like_server() {
  [ -z "${DISPLAY:-}" ] && [ -z "${WAYLAND_DISPLAY:-}" ] || return 1
  [ "$(systemctl get-default 2>/dev/null)" = graphical.target ] && return 1
  for sessions in /usr/share/xsessions /usr/share/wayland-sessions; do
    ls "$sessions"/*.desktop >/dev/null 2>&1 && return 1
  done
  return 0
}

choose_linux_mode() {
  [ "$MODE" != host ] || return 0
  unavailable=$(app_unavailable)
  if [ -n "$unavailable" ]; then
    [ "$MODE" = ask ] || fail "the desktop app cannot go on this machine: $unavailable; add --host for the core"
    note "No desktop app here: $unavailable, so this installs the core."
    say ""
    MODE=host
    return 0
  fi
  [ "$MODE" = ask ] || return 0
  if looks_like_server; then guess=host; else guess=app; fi
  if ! interactive; then
    MODE=$guess
    if [ "$guess" = host ]; then
      note "No desktop here, so this installs the core. Add --app for the desktop app."
      say ""
    fi
    return 0
  fi
  if [ "$guess" = host ]; then
    say "  This machine has no desktop, so it looks like a server."
    default=1
  else
    default=2
  fi
  say ""
  say "    ${BOLD}1${RESET}  Core only     ${DIM}runs agents here, starts at boot, reachable from your Mac and phone${RESET}"
  say "    ${BOLD}2${RESET}  Desktop app   ${DIM}the full Sikemux window${RESET}"
  case "$(ask "Install [$default]:" "$default")" in
    1) MODE=host ;;
    2) MODE=app ;;
    *) fail "pick 1 or 2" ;;
  esac
  say ""
}

latest_version() {
  case "$CHANNEL" in
    stable)
      url=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "$REPO/releases/latest") || fail "could not reach GitHub"
      printf '%s' "${url##*/tag/v}"
      ;;
    nightly)
      curl -fsSL "$REPO/releases/download/nightly/latest.json" |
        sed -n 's/^ *"version": *"\([^"]*\)".*/\1/p' | head -n 1
      ;;
    *) fail "SIKEMUX_CHANNEL must be stable or nightly, not $CHANNEL" ;;
  esac
}

size_of() {
  awk -v bytes="$(wc -c <"$1" | tr -d ' ')" 'BEGIN { printf "%.1f MB", bytes / 1048576 }'
}

download() {
  step "Downloading $2" curl -fsSL -o "$TMP/$1" "$REPO/releases/download/v$VERSION/$1"
  done_step "Downloaded $2" "$(size_of "$TMP/$1")"
}

as_root() {
  if [ "$(id -u)" -eq 0 ]; then "$@"
  elif command -v sudo >/dev/null 2>&1; then sudo "$@"
  else fail "run this as root, or install sudo"
  fi
}

confirm_leaving_nightly() {
  [ "$CHANNEL" = stable ] || return 0
  installed=$(plutil -extract CFBundleShortVersionString raw "$1/Contents/Info.plist" 2>/dev/null) || return 0
  case "$installed" in
    *-nightly.*) ;;
    *) return 0 ;;
  esac
  say "  This replaces nightly $installed with stable $VERSION."
  note "To stay on nightly, run this with SIKEMUX_CHANNEL=nightly."
  interactive || { say ""; return 0; }
  case "$(ask "Replace it? [y/N]" n)" in
    [Yy]*) say "" ;;
    *)
      say ""
      note "Left nightly $installed in place."
      say ""
      exit 0
      ;;
  esac
}

install_mac() {
  # Under Rosetta, uname reports x86_64 even on Apple Silicon.
  [ "$(sysctl -n hw.optional.arm64 2>/dev/null)" = 1 ] || fail "Sikemux needs a Mac with Apple Silicon"
  if pgrep -xq Sikemux; then fail "quit Sikemux first, then run this again"; fi
  dest=/Applications
  [ -w "$dest" ] || { dest="$HOME/Applications"; mkdir -p "$dest"; }
  confirm_leaving_nightly "$dest/Sikemux.app"
  download Sikemux.app.tar.gz "the app"
  tar -xzf "$TMP/Sikemux.app.tar.gz" -C "$TMP"
  rm -rf "$dest/Sikemux.app"
  mv "$TMP/Sikemux.app" "$dest/Sikemux.app"
  done_step "Installed Sikemux" "$(home_path "$dest")"
  say ""
  open "$dest/Sikemux.app"
}

# An AppImage runs from a mount any program can make, so the core could not tell its
# window from an agent's program. Unpacked where only root writes, beside the core, it can.
install_appimage() {
  file=$1
  download "$file" "the app"
  chmod +x "$TMP/$file"
  # shellcheck disable=SC2016
  step "Unpacking the app" sh -c 'cd "$1" && "$2" --appimage-extract' sh "$TMP" "$TMP/$file"
  # shellcheck disable=SC2016
  step "Installing the app" as_root sh -c '
    set -e
    rm -rf /usr/local/lib/sikemux/.app.new /usr/local/lib/sikemux/.app.old
    cp -R "$1" /usr/local/lib/sikemux/.app.new
    chown -R root:root /usr/local/lib/sikemux/.app.new
    chmod -R go-w /usr/local/lib/sikemux/.app.new
    chmod 755 /usr/local/lib/sikemux/.app.new
    if [ -e /usr/local/lib/sikemux/app ]; then mv /usr/local/lib/sikemux/app /usr/local/lib/sikemux/.app.old; fi
    mv /usr/local/lib/sikemux/.app.new /usr/local/lib/sikemux/app
    rm -rf /usr/local/lib/sikemux/.app.old
  ' sh "$TMP/squashfs-root"
  done_step "Installed Sikemux" "/usr/local/lib/sikemux/app"
  add_to_launcher /usr/local/lib/sikemux/app
}

# App launchers list only apps with a .desktop entry, and an unpacked AppImage installs none.
add_to_launcher() {
  app=$1
  data="${XDG_DATA_HOME:-$HOME/.local/share}"
  mkdir -p "$data/applications" "$data/icons"
  [ -d "$app/usr/share/icons" ] && cp -R "$app/usr/share/icons/." "$data/icons/"
  for entry in "$app"/*.desktop; do
    [ -f "$entry" ] || continue
    sed "s|^Exec=[^ ]*|Exec=\"$app/AppRun\"|" "$entry" >"$data/applications/${entry##*/}"
    command -v xdg-mime >/dev/null 2>&1 && xdg-mime default "${entry##*/}" x-scheme-handler/sikemux 2>/dev/null || true
  done
  command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$data/applications" 2>/dev/null || true
  command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q "$data/icons/hicolor" 2>/dev/null || true
  done_step "Added to the app launcher"
}

install_linux() {
  file=$(app_file)
  # sudo asks for a password here, if it needs one, before a spinner holds the line.
  if [ "$(id -u)" -ne 0 ] && command -v sudo >/dev/null 2>&1; then sudo -n true 2>/dev/null || sudo -v; fi
  case "$file" in
    *.deb)
      download "$file" "the app"
      step "Installing with apt" as_root apt-get install -y "$TMP/$file"
      done_step "Installed Sikemux" "with apt"
      ;;
    *.pkg.tar.zst)
      download "$file" "the app"
      # The other channel's package, or the Omarchy repo's, holds the same files.
      owner=$(pacman -Qqo /usr/lib/sikemux/sikemux-host 2>/dev/null || true)
      if [ -n "$owner" ] && [ "$owner" != "${file%%-[0-9]*}" ]; then
        step "Removing $owner" as_root pacman -Rdd --noconfirm "$owner"
      fi
      step "Installing with pacman" as_root pacman -U --noconfirm "$TMP/$file"
      done_step "Installed Sikemux" "with pacman"
      add_pacman_repository
      ;;
    *)
      # Without apt, and on arm64 Arch, which has no pacman package, the app is the AppImage.
      install_guarded_core "$ARCH"
      install_appimage "$file"
      link_cli
      ;;
  esac
  say ""
}

# Root replaces the core, and the app unpacked beside it, when the person's core asks over this socket. It
# takes nothing from the caller but who they are and the channel they follow,
# and installs only a release signed with Sikemux's updater key.
UPDATE_UNITS='
set -e
cat >/etc/systemd/system/sikemux-update.socket <<"UNIT"
[Unit]
Description=Sikemux core updates

[Socket]
ListenStream=/run/sikemux-update.sock
SocketMode=0666
Accept=yes
MaxConnections=4

[Install]
WantedBy=sockets.target
UNIT
cat >/etc/systemd/system/sikemux-update@.service <<"UNIT"
[Unit]
Description=Install a newer signed Sikemux core

[Service]
ExecStart=/usr/local/lib/sikemux/sikemux-host update --install
StandardInput=socket
StandardOutput=socket
StandardError=journal
RuntimeMaxSec=60min
MemoryMax=512M
ProtectSystem=strict
ReadWritePaths=/usr/local/lib/sikemux
ProtectHome=yes
PrivateTmp=yes
PrivateDevices=yes
NoNewPrivileges=yes
CapabilityBoundingSet=CAP_CHOWN CAP_FSETID CAP_DAC_READ_SEARCH CAP_DAC_OVERRIDE CAP_FOWNER
RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6
RestrictNamespaces=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectControlGroups=yes
LockPersonality=yes
SystemCallArchitectures=native
UNIT
systemctl daemon-reload
systemctl enable --now sikemux-update.socket >/dev/null 2>&1
'

# Points pacman at the latest stable release, so pacman -Syu brings every stable after this one.
add_pacman_repository() {
  [ "$CHANNEL" = stable ] || return 0
  grep -q '^\[sikemux\]' /etc/pacman.conf && return 0
  curl -fsSLI -o /dev/null "$REPO/releases/download/v$VERSION/sikemux.db" 2>/dev/null || return 0
  curl -fsSL -o "$TMP/sikemux-archive.asc" "$REPO/releases/download/v$VERSION/sikemux-archive.asc" || return 0
  step "Trusting Sikemux's package key" as_root sh -c \
    "pacman-key --add '$TMP/sikemux-archive.asc' && pacman-key --lsign-key $PACKAGES_KEY"
  printf '\n[sikemux]\nSigLevel = Required DatabaseRequired\nServer = %s/releases/latest/download\n' "$REPO" |
    as_root tee -a /etc/pacman.conf >/dev/null
  done_step "Added Sikemux's repository" "pacman -Syu updates it"
}

# The core keeps its secrets in a folder only the group sikemux may enter,
# and runs setgid to it; you stay out of the group, so your agents do too.
# A package sets this up itself; an AppImage cannot, since it runs from a mount
# that ignores setgid, so the installed app starts this core instead of its own.
install_guarded_core() {
  user=$(id -un)
  if id -nG "$user" | tr ' ' '\n' | grep -qx sikemux; then
    fail "$user is in the sikemux group, which would let your agents read the core's secrets; remove it with: sudo gpasswd -d $user sikemux"
  fi
  # sudo asks for a password here, if it needs one, before a spinner holds the line.
  if [ "$(id -u)" -ne 0 ] && command -v sudo >/dev/null 2>&1; then sudo -n true 2>/dev/null || sudo -v; fi
  asset="sikemux-host-${VERSION}-linux-$1.tar.gz"
  download "$asset" "the core"
  tar -xzf "$TMP/$asset" -C "$TMP"
  # shellcheck disable=SC2016
  step "Setting up the core's secret store" as_root sh -c '
    set -e
    getent group sikemux >/dev/null || groupadd --system sikemux
    install -d -o root -g root -m 755 /usr/local/lib/sikemux
    install -o root -g sikemux -m 2755 "$1" /usr/local/lib/sikemux/.sikemux-host.new
    # On Ubuntu 26.04, install drops the setgid bit when it sets the owner.
    chmod 2755 /usr/local/lib/sikemux/.sikemux-host.new
    mv -f /usr/local/lib/sikemux/.sikemux-host.new /usr/local/lib/sikemux/sikemux-host
    install -d -o root -g sikemux -m 770 /var/lib/sikemux
  ' sh "$TMP/sikemux-host"
  if [ -d /run/systemd/system ]; then
    step "Setting up updates" as_root sh -c "$UPDATE_UNITS"
    done_step "Updates itself" "signed releases"
  fi
}

# Puts `sikemux` on PATH, run as the core, for a person and for Sikemux over SSH alike.
link_cli() {
  if [ "$(id -u)" -eq 0 ]; then bin=/usr/local/bin; else bin="$HOME/.local/bin"; fi
  mkdir -p "$bin"
  ln -sf /usr/local/lib/sikemux/sikemux-host "$bin/sikemux"
}

# Says so plainly, instead of leaving the loader's or the package manager's error.
# Prints nothing when glibc is new enough, "musl", or the version it found.
glibc_short_of() {
  have=$(getconf GNU_LIBC_VERSION 2>/dev/null | sed -n 's/^glibc //p')
  if [ -z "$have" ]; then
    if ldd --version 2>&1 | grep -qi musl; then echo musl; fi
    return 0
  fi
  [ "$(printf '%s\n%s\n' "$1" "$have" | sort -V | head -n 1)" = "$1" ] || echo "$have"
}

need_glibc() {
  have=$(glibc_short_of "$OLDEST_GLIBC")
  case "$have" in
    "") ;;
    musl) fail "a Sikemux host needs glibc $OLDEST_GLIBC or later, and this system uses musl" ;;
    *) fail "a Sikemux host needs glibc $OLDEST_GLIBC or later, and this system has $have; Ubuntu 22.04, Debian 12, RHEL 9 and Amazon Linux 2023 or newer have it" ;;
  esac
}

# The package the desktop app comes in on this machine.
app_file() {
  if command -v apt-get >/dev/null 2>&1; then
    echo "Sikemux_${VERSION}_${DEB_ARCH}.deb"
  elif [ "$ARCH" = x86_64 ] && command -v pacman >/dev/null 2>&1; then
    # packaging/arch/PKGBUILD names it: pacman sorts 0.6.1nightly.6 below 0.6.1.
    case "$VERSION" in
      *-*) name=sikemux-nightly-bin ;;
      *) name=sikemux-bin ;;
    esac
    echo "$name-$(printf '%s' "$VERSION" | tr -d -)-1-x86_64.pkg.tar.zst"
  else
    echo "Sikemux_${VERSION}_${DEB_ARCH}.AppImage"
  fi
}

# Why this machine cannot take the desktop app from this release; nothing when it can.
app_unavailable() {
  if [ -z "$ARCH" ]; then
    echo "it runs on x86_64 and arm64 only"
    return 0
  fi
  have=$(glibc_short_of "$OLDEST_APP_GLIBC")
  case "$have" in
    "") ;;
    musl) echo "it needs glibc $OLDEST_APP_GLIBC or later, and this system uses musl"; return 0 ;;
    *) echo "it needs glibc $OLDEST_APP_GLIBC or later (Ubuntu 22.04, Debian 12 or newer), and this system has $have"; return 0 ;;
  esac
  file=$(app_file)
  found=0
  curl -fsLI -o /dev/null "$REPO/releases/download/v$VERSION/$file" || found=$?
  # 22 is an HTTP error such as 404, 37 a missing file:// path; anything else is left to the download.
  case $found in
    22 | 37) echo "$VERSION has no $file" ;;
  esac
}

install_host() {
  [ -n "$ARCH" ] || fail "a Sikemux host needs an x86_64 or arm64 machine"
  need_glibc
  if [ "$(id -u)" -eq 0 ]; then bin=/usr/local/bin; else bin="$HOME/.local/bin"; fi
  previous=""
  [ -x "$bin/sikemux" ] && previous=$("$bin/sikemux" --version 2>/dev/null | sed 's/^sikemux //')
  install_guarded_core "$ARCH"
  link_cli
  user=$(id -un)
  if [ "$(id -u)" -eq 0 ]; then
    note "Running as root, agents run as root too and can read anything; use your own account to keep them out of the core's secrets."
  fi
  updated=false
  if [ -n "$previous" ] && [ "$previous" != "$VERSION" ]; then
    updated=true
    done_step "Updated sikemux $previous → $VERSION" "$(home_path "$bin")"
  else
    done_step "Installed sikemux" "$(home_path "$bin")"
  fi

  if systemctl --user show-environment >/dev/null 2>&1; then
    loginctl enable-linger "$user" 2>/dev/null || as_root loginctl enable-linger "$user" >/dev/null 2>&1 || true
    step "Starting the core" "$bin/sikemux" enable
    done_step "Starts at boot" "systemd user service"
    if [ "$(loginctl show-user "$user" -p Linger --value 2>/dev/null)" = yes ]; then
      done_step "Keeps running after you log out" "linger on"
    else
      note "To keep it running after you log out, run: loginctl enable-linger"
    fi
  else
    note "No systemd user session here, so the core starts on demand, not at boot."
  fi

  if $updated && "$bin/sikemux" status >/dev/null 2>&1; then
    note "The running core keeps $previous until you run: sikemux restart"
  fi
  login_path=$("${SHELL:-/bin/sh}" -lc "printf %s \"\$PATH\"" </dev/null 2>/dev/null || true)
  case ":$PATH:$login_path:" in
    *":$bin:"*) ;;
    *) note "Add $(home_path "$bin") to your PATH to run sikemux from any shell." ;;
  esac

  if "$bin/sikemux" status --json 2>/dev/null | grep -q '"account": "'; then
    say ""
    say "  This host is signed in: your Mac and phone reach it from Sikemux now."
    say ""
    return 0
  fi
  if [ "$SIGN_IN" = later ]; then
    say ""
    return 0
  fi
  say ""
  say "  Next, sign this host in to your Sikemux account. Your Mac and phone"
  say "  reach it from Sikemux as soon as it is."
  if ! (: </dev/tty) 2>/dev/null; then
    say ""
    note "Sign in with: sikemux login"
    say ""
    return 0
  fi
  if ! $YES; then
    case "$(ask "Sign in now? [Y/n]" y)" in
      [Nn]*)
        say ""
        note "Sign in later with: sikemux login"
        say ""
        return 0
        ;;
    esac
  fi
  "$bin/sikemux" login </dev/tty >/dev/tty 2>&1 || true
}

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
VERSION=$(latest_version)
[ -n "$VERSION" ] || fail "could not find the latest $CHANNEL version"

case "$(uname -s)" in
  Darwin) platform="macOS" ;;
  Linux) platform="Linux" ;;
  *) fail "Sikemux runs on macOS and Linux" ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) ARCH=x86_64 DEB_ARCH=amd64 ;;
  aarch64 | arm64) ARCH=aarch64 DEB_ARCH=arm64 ;;
  *) ARCH="" DEB_ARCH="" ;;
esac
handed_over=false
[ "${SIKEMUX_INSTALLER:-}" = "$VERSION" ] && handed_over=true
if ! $handed_over; then
  say ""
  say "  ${ACCENT}${BOLD}sikemux${RESET}  ${DIM}$VERSION · $platform $(uname -m)${RESET}"
  say ""
fi
[ "$platform" = Linux ] && choose_linux_mode

# The nightly's own installer takes over, told what was chosen here.
if [ "$CHANNEL" = nightly ] && ! $handed_over; then
  curl -fsSL -o "$TMP/install.sh" "$REPO/releases/download/v$VERSION/install.sh" ||
    fail "v$VERSION has no installer"
  [ "$platform" = Linux ] && set -- "$@" "--$MODE"
  SIKEMUX_INSTALLER=$VERSION sh "$TMP/install.sh" "$@" </dev/null
  exit $?
fi

case "$(uname -s)" in
  Darwin)
    [ "$MODE" != host ] || fail "--host installs the core on a Linux server; on a Mac, the app runs it"
    install_mac
    ;;
  Linux)
    choose_linux_mode
    if [ "$MODE" = host ]; then install_host; else install_linux; fi
    ;;
esac
