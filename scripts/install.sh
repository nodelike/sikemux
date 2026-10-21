#!/bin/sh
# Installs Sikemux: curl -fsSL https://sikemux.com/install | sh
# Each release carries the installer made for its files, and sikemux.com serves
# the latest stable one; a nightly install hands over to that nightly's own.
# On Linux it asks whether to install the desktop app or just the core, which
# runs agents on a server; `| sh -s -- --host` or `--app` picks without asking,
# and `--yes` takes every default. Set SIKEMUX_CHANNEL=nightly for the nightly build.
set -eu

REPO="https://github.com/nodelike/sikemux"
CHANNEL="${SIKEMUX_CHANNEL:-stable}"
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
# A finished step: what happened, then where or how in grey.
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
  [ "$MODE" = ask ] || return 0
  if [ "$(uname -m)" != x86_64 ] && [ "$(uname -m)" != amd64 ]; then
    note "The desktop app is x86_64 only, so this installs the core."
    say ""
    MODE=host
    return 0
  fi
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

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
VERSION=$(latest_version)
[ -n "$VERSION" ] || fail "could not find the latest $CHANNEL version"

if [ "$CHANNEL" = nightly ] && [ "${SIKEMUX_INSTALLER:-}" != "$VERSION" ]; then
  curl -fsSL -o "$TMP/install.sh" "$REPO/releases/download/v$VERSION/install.sh" ||
    fail "v$VERSION has no installer"
  SIKEMUX_INSTALLER=$VERSION sh "$TMP/install.sh" "$@" </dev/null
  exit $?
fi

case "$(uname -s)" in
  Darwin) platform="macOS" ;;
  Linux) platform="Linux" ;;
  *) fail "Sikemux runs on macOS and Linux" ;;
esac
say ""
say "  ${ACCENT}${BOLD}sikemux${RESET}  ${DIM}$VERSION · $platform $(uname -m)${RESET}"
say ""

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

install_mac() {
  [ "$(uname -m)" = arm64 ] || fail "Sikemux needs a Mac with Apple Silicon"
  if pgrep -xq Sikemux; then fail "quit Sikemux first, then run this again"; fi
  download Sikemux.app.tar.gz "the app"
  tar -xzf "$TMP/Sikemux.app.tar.gz" -C "$TMP"
  dest=/Applications
  [ -w "$dest" ] || { dest="$HOME/Applications"; mkdir -p "$dest"; }
  rm -rf "$dest/Sikemux.app"
  mv "$TMP/Sikemux.app" "$dest/Sikemux.app"
  done_step "Installed Sikemux" "$(home_path "$dest")"
  say ""
  open "$dest/Sikemux.app"
}

install_linux() {
  [ "$(uname -m)" = x86_64 ] || fail "the desktop app needs an x86_64 machine; add --host for the core"
  # sudo asks for a password here, if it needs one, before a spinner holds the line.
  if [ "$(id -u)" -ne 0 ] && command -v sudo >/dev/null 2>&1; then sudo -n true 2>/dev/null || sudo -v; fi
  if command -v apt-get >/dev/null 2>&1; then
    download "Sikemux_${VERSION}_amd64.deb" "the app"
    step "Installing with apt" as_root apt-get install -y "$TMP/Sikemux_${VERSION}_amd64.deb"
    done_step "Installed Sikemux" "with apt"
  elif command -v dnf >/dev/null 2>&1; then
    download "Sikemux-${VERSION}-1.x86_64.rpm" "the app"
    step "Installing with dnf" as_root dnf install -y "$TMP/Sikemux-${VERSION}-1.x86_64.rpm"
    done_step "Installed Sikemux" "with dnf"
  elif command -v zypper >/dev/null 2>&1; then
    download "Sikemux-${VERSION}-1.x86_64.rpm" "the app"
    step "Installing with zypper" as_root zypper --non-interactive install --allow-unsigned-rpm "$TMP/Sikemux-${VERSION}-1.x86_64.rpm"
    done_step "Installed Sikemux" "with zypper"
  else
    download "Sikemux_${VERSION}_amd64.AppImage" "the app"
    mkdir -p "$HOME/Applications"
    mv "$TMP/Sikemux_${VERSION}_amd64.AppImage" "$HOME/Applications/Sikemux.AppImage"
    chmod +x "$HOME/Applications/Sikemux.AppImage"
    done_step "Installed Sikemux" "$(home_path "$HOME/Applications/Sikemux.AppImage")"
  fi
  say ""
}

install_host() {
  case "$(uname -m)" in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    *) fail "a Sikemux host needs an x86_64 or arm64 machine" ;;
  esac
  user=$(id -un)
  if id -nG "$user" | tr ' ' '\n' | grep -qx sikemux; then
    fail "$user is in the sikemux group, which would let your agents read the core's secrets; remove it with: sudo gpasswd -d $user sikemux"
  fi
  # sudo asks for a password here, if it needs one, before a spinner holds the line.
  if [ "$(id -u)" -ne 0 ] && command -v sudo >/dev/null 2>&1; then sudo -n true 2>/dev/null || sudo -v; fi
  asset="sikemux-host-${VERSION}-linux-${arch}.tar.gz"
  download "$asset" "the core"
  tar -xzf "$TMP/$asset" -C "$TMP"
  if [ "$(id -u)" -eq 0 ]; then bin=/usr/local/bin; else bin="$HOME/.local/bin"; fi
  mkdir -p "$bin"
  previous=""
  [ -x "$bin/sikemux" ] && previous=$("$bin/sikemux" --version 2>/dev/null | sed 's/^sikemux //')
  # The core keeps its secrets in a folder only the group sikemux may enter,
  # and runs setgid to it; you stay out of the group, so your agents do too.
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
  ln -sf /usr/local/lib/sikemux/sikemux-host "$bin/sikemux"
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
  case ":$PATH:" in
    *":$bin:"*) ;;
    *) note "Add $(home_path "$bin") to your PATH to run sikemux from any shell." ;;
  esac

  if "$bin/sikemux" status --json 2>/dev/null | grep -q '"account": "'; then
    say ""
    say "  This host is signed in: your Mac and phone reach it from Sikemux now."
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
  say ""
  "$bin/sikemux" login </dev/tty >/dev/tty 2>&1 || true
}

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
