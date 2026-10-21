#!/usr/bin/env bash
# Prepares citadel to run the Sikemux API and web app. Run as root from a copy of this folder:
#   ./setup-citadel.sh <deploy-public-key-file>
# Safe to run again: each step checks what is already there.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=lib.sh
. "$here/lib.sh"
deploy_key="${1:?usage: setup-citadel.sh <deploy-public-key-file>}"
[ "$(id -u)" = 0 ] || { echo "run as root" >&2; exit 1; }

PG_VERSION=17
PG_CONF="/etc/postgresql/$PG_VERSION/main"
PG_ADMIN=nodelike

step() { printf '\n== %s\n' "$*"; }

step "kernel"
install -m 644 "$here/sysctl.conf" /etc/sysctl.d/60-sikemux.conf
sysctl --quiet --load /etc/sysctl.d/60-sikemux.conf

step "users"
id sikemux >/dev/null 2>&1 ||
  useradd --system --no-create-home --home-dir /nonexistent --shell /usr/sbin/nologin sikemux
id sikemux-deploy >/dev/null 2>&1 ||
  useradd --system --create-home --home-dir /var/lib/sikemux-deploy --shell /bin/bash sikemux-deploy
passwd -l sikemux-deploy >/dev/null

step "deploy key, limited to receiving releases and phone updates"
install -d -m 700 -o sikemux-deploy -g sikemux-deploy /var/lib/sikemux-deploy/.ssh
printf 'restrict,command="/usr/local/lib/sikemux/receive-release" %s\n' "$(tr -d '\n' <"$deploy_key")" \
  >/var/lib/sikemux-deploy/.ssh/authorized_keys
chown sikemux-deploy:sikemux-deploy /var/lib/sikemux-deploy/.ssh/authorized_keys
chmod 600 /var/lib/sikemux-deploy/.ssh/authorized_keys
install -D -m 644 -o root -g root "$here/lib.sh" /usr/local/lib/sikemux/lib.sh
install -D -m 755 -o root -g root "$here/receive-release" /usr/local/lib/sikemux/receive-release

cat >/etc/sudoers.d/sikemux-deploy <<'EOF'
sikemux-deploy ALL=(root) NOPASSWD: /usr/bin/systemctl restart sikemux-api, /usr/bin/journalctl -u sikemux-api -n 50 --no-pager
EOF
chmod 440 /etc/sudoers.d/sikemux-deploy
visudo -cf /etc/sudoers.d/sikemux-deploy

step "folders"
install -d -m 755 -o sikemux-deploy -g sikemux-deploy /srv/sikemux /srv/sikemux/releases
# Phone update assets: written only by the deploy user, read by Caddy, which serves them as they are.
install -d -m 755 -o sikemux-deploy -g sikemux-deploy /srv/sikemux/updates /srv/sikemux/updates/assets
install -d -m 750 -o root -g sikemux /etc/sikemux
if [ ! -f /etc/sikemux/api.env ]; then
  cat >/etc/sikemux/api.env <<EOF
HOST=127.0.0.1
PORT=4000
DATABASE_URL=$SIKEMUX_DATABASE_URL
APP_ORIGIN=https://app.sikemux.com
CLERK_ISSUER=https://clerk.sikemux.com
CLERK_MAC_CLIENT_ID=I4QXaIf3c7zntZR8
LOG_LEVEL=info
EOF
fi
chown root:sikemux /etc/sikemux/api.env
chmod 640 /etc/sikemux/api.env
grep -q '^GEOIP_DIR=' /etc/sikemux/api.env ||
  echo 'GEOIP_DIR=/var/lib/sikemux-places' >>/etc/sikemux/api.env

step "database, reachable only by the sikemux users through the local socket"
psql -U "$PG_ADMIN" -d postgres -v ON_ERROR_STOP=1 -q <<'SQL'
select 'create role sikemux login' where not exists (select from pg_roles where rolname = 'sikemux') \gexec
select 'create database sikemux owner sikemux' where not exists (select from pg_database where datname = 'sikemux') \gexec
revoke all on database sikemux from public;
SQL

if ! grep -q '^# sikemux: begin' "$PG_CONF/pg_hba.conf"; then
  cp "$PG_CONF/pg_hba.conf" "$PG_CONF/pg_hba.conf.bak.$(date +%Y%m%d%H%M%S)"
  rules="$(mktemp)"
  cat >"$rules" <<'EOF'
# sikemux: begin
# Only the sikemux service and its deploy user reach the sikemux database, as the sikemux role,
# through the local socket. These come first, so no broader rule below applies to it.
local   sikemux         sikemux                                 peer map=sikemux
local   sikemux         all                                     reject
host    sikemux         all             all                     reject
# sikemux: end

EOF
  first_rule="$(grep -n '^[^#[:space:]]' "$PG_CONF/pg_hba.conf" | head -1 | cut -d: -f1)"
  sed -i "$((first_rule - 1))r $rules" "$PG_CONF/pg_hba.conf"
  rm -f "$rules"
fi
if ! grep -q '^sikemux ' "$PG_CONF/pg_ident.conf"; then
  printf 'sikemux         sikemux                 sikemux\nsikemux         sikemux-deploy          sikemux\n' >>"$PG_CONF/pg_ident.conf"
fi
systemctl reload "postgresql@$PG_VERSION-main"
if ! cmp -s "$here/postgresql.conf" "$PG_CONF/conf.d/sikemux.conf"; then
  install -m 644 "$here/postgresql.conf" "$PG_CONF/conf.d/sikemux.conf"
  # shared_buffers is read only at start.
  systemctl restart "postgresql@$PG_VERSION-main"
fi
sudo -u sikemux-deploy psql "$SIKEMUX_DATABASE_URL" -Atc 'select current_user' |
  grep -qx sikemux
if psql -U "$PG_ADMIN" -d sikemux -c 'select 1' >/dev/null 2>&1; then
  echo "other roles can still reach the sikemux database" >&2
  exit 1
fi

step "service"
install -m 644 "$here/sikemux-api.service" /etc/systemd/system/sikemux-api.service
install -m 644 "$here/sikemux-purge.service" /etc/systemd/system/sikemux-purge.service
install -m 644 "$here/sikemux-purge.timer" /etc/systemd/system/sikemux-purge.timer
install -m 644 "$here/sikemux-places.service" /etc/systemd/system/sikemux-places.service
install -m 644 "$here/sikemux-places.timer" /etc/systemd/system/sikemux-places.timer
install -D -m 755 -o root -g root "$here/update-places" /usr/local/lib/sikemux/update-places
systemctl daemon-reload
systemctl enable sikemux-api >/dev/null
systemctl enable --now sikemux-purge.timer >/dev/null
systemctl enable --now sikemux-places.timer >/dev/null
[ -f /var/lib/sikemux-places/dbip-asn-lite.mmdb ] || systemctl start sikemux-places

step "backups: nightly, encrypted to an offline age key, sent to R2"
if ! command -v age >/dev/null || ! command -v rclone >/dev/null; then
  DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends age rclone >/dev/null
fi
id sikemux-backup >/dev/null 2>&1 ||
  useradd --system --no-create-home --home-dir /nonexistent --shell /usr/sbin/nologin sikemux-backup
install -D -m 755 -o root -g root "$here/backup-database" /usr/local/lib/sikemux/backup-database
install -D -m 755 -o root -g root "$here/restore-database" /usr/local/lib/sikemux/restore-database
if [ ! -f /etc/sikemux/backup.env ]; then
  cat >/etc/sikemux/backup.env <<'EOF'
R2_ACCOUNT_ID=
R2_ACCESS_KEY_ID=
R2_SECRET_ACCESS_KEY=
R2_BUCKET=sikemux-backups
EOF
fi
chown root:root /etc/sikemux/backup.env
chmod 600 /etc/sikemux/backup.env
if [ ! -f /etc/sikemux/backup-recipients.txt ]; then
  printf '# age public keys (age1...), one per line. Their private keys never come to this server.\n' \
    >/etc/sikemux/backup-recipients.txt
fi
chown root:root /etc/sikemux/backup-recipients.txt
chmod 644 /etc/sikemux/backup-recipients.txt

psql -U "$PG_ADMIN" -d postgres -v ON_ERROR_STOP=1 -q <<'SQL'
select 'create role sikemux_backup login' where not exists (select from pg_roles where rolname = 'sikemux_backup') \gexec
grant pg_read_all_data to sikemux_backup;
grant connect on database sikemux to sikemux_backup;
SQL
if ! grep -q '^# sikemux-backup: begin' "$PG_CONF/pg_hba.conf"; then
  cp "$PG_CONF/pg_hba.conf" "$PG_CONF/pg_hba.conf.bak.$(date +%Y%m%d%H%M%S)"
  rules="$(mktemp)"
  cat >"$rules" <<'EOF'
# sikemux-backup: begin
# The nightly backup reads the sikemux database as sikemux_backup, which can read but not write.
local   sikemux         sikemux_backup                          peer map=sikemux-backup
# sikemux-backup: end

EOF
  sikemux_rules="$(grep -n '^# sikemux: begin' "$PG_CONF/pg_hba.conf" | cut -d: -f1)"
  sed -i "$((sikemux_rules - 1))r $rules" "$PG_CONF/pg_hba.conf"
  rm -f "$rules"
fi
grep -q '^sikemux-backup ' "$PG_CONF/pg_ident.conf" ||
  printf 'sikemux-backup  sikemux-backup          sikemux_backup\n' >>"$PG_CONF/pg_ident.conf"
systemctl reload "postgresql@$PG_VERSION-main"
sudo -u sikemux-backup psql 'postgresql://sikemux_backup@%2Fvar%2Frun%2Fpostgresql/sikemux' -Atc 'select current_user' |
  grep -qx sikemux_backup

install -m 644 "$here/sikemux-backup.service" /etc/systemd/system/sikemux-backup.service
install -m 644 "$here/sikemux-backup.timer" /etc/systemd/system/sikemux-backup.timer
systemctl daemon-reload
systemctl enable --now sikemux-backup.timer >/dev/null
grep -q '^age1' /etc/sikemux/backup-recipients.txt ||
  echo "backups will fail until an age public key is in /etc/sikemux/backup-recipients.txt" >&2
grep -q '^R2_SECRET_ACCESS_KEY=.' /etc/sikemux/backup.env ||
  echo "backups will fail until the R2 credentials are in /etc/sikemux/backup.env" >&2

step "caddy"
install -d -m 755 /etc/caddy/sites
install -m 644 "$here/sikemux.caddy" /etc/caddy/sites/sikemux.caddy
grep -qx 'import /etc/caddy/sites/\*.caddy' /etc/caddy/Caddyfile ||
  printf '\nimport /etc/caddy/sites/*.caddy\n' >>/etc/caddy/Caddyfile
sudo -u caddy caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile >/dev/null
systemctl reload caddy

# n0's release of the relay, the same version as the iroh the apps are built with
# (src-tauri/Cargo.lock). Bump it together with them.
RELAY_VERSION=1.3.0
case "$(uname -m)" in
  x86_64)
    RELAY_TARGET=x86_64-unknown-linux-musl
    RELAY_SHA256=677f4c62342a6ba8044459b5fd4302f2b1dcb8402542072e3a4ade5039bc0b9e
    ;;
  aarch64)
    RELAY_TARGET=aarch64-unknown-linux-musl
    RELAY_SHA256=dc4b9d620642026966d498763ec8ba3a6eefde8d39994b2d63e3d15d7af770f8
    ;;
  *) echo "no relay build for $(uname -m)" >&2; exit 1 ;;
esac

step "relay: iroh-relay $RELAY_VERSION behind Caddy at relay.sikemux.com, QUIC address discovery on udp/7842"
id sikemux-relay >/dev/null 2>&1 ||
  useradd --system --no-create-home --home-dir /nonexistent --shell /usr/sbin/nologin sikemux-relay
relay_changed=0
if ! /usr/local/lib/sikemux/iroh-relay --version 2>/dev/null | grep -qx "iroh-relay $RELAY_VERSION"; then
  download="$(mktemp -d)"
  curl -fsSL -o "$download/relay.tar.gz" \
    "https://github.com/n0-computer/iroh/releases/download/v$RELAY_VERSION/iroh-relay-v$RELAY_VERSION-$RELAY_TARGET.tar.gz"
  echo "$RELAY_SHA256  $download/relay.tar.gz" | sha256sum --check --quiet
  tar -xzf "$download/relay.tar.gz" -C "$download" ./iroh-relay
  install -D -m 755 -o root -g root "$download/iroh-relay" /usr/local/lib/sikemux/iroh-relay.new
  mv -f /usr/local/lib/sikemux/iroh-relay.new /usr/local/lib/sikemux/iroh-relay
  rm -rf "$download"
  relay_changed=1
fi
install -d -m 750 -o root -g sikemux-relay /etc/sikemux-relay
# One token, so the API knows the relay is the one asking which devices may use it.
if [ ! -f /etc/sikemux-relay/relay.env ]; then
  printf 'IROH_RELAY_HTTP_BEARER_TOKEN=%s\n' "$(openssl rand -hex 32)" >/etc/sikemux-relay/relay.env
  relay_changed=1
fi
chown root:sikemux-relay /etc/sikemux-relay/relay.env
chmod 640 /etc/sikemux-relay/relay.env
relay_token="$(sed -n 's/^IROH_RELAY_HTTP_BEARER_TOKEN=//p' /etc/sikemux-relay/relay.env)"
if ! grep -qx "RELAY_ACCESS_TOKEN=$relay_token" /etc/sikemux/api.env; then
  sed -i '/^RELAY_ACCESS_TOKEN=/d' /etc/sikemux/api.env
  echo "RELAY_ACCESS_TOKEN=$relay_token" >>/etc/sikemux/api.env
  systemctl restart sikemux-api
fi
unset relay_token
cmp -s "$here/sikemux-relay.toml" /etc/sikemux-relay/relay.toml || relay_changed=1
install -m 640 -o root -g sikemux-relay "$here/sikemux-relay.toml" /etc/sikemux-relay/relay.toml
install -m 755 -o root -g root "$here/relay-certificate" /usr/local/lib/sikemux/relay-certificate
install -m 755 -o root -g root "$here/relay-health" /usr/local/lib/sikemux/relay-health
for unit in sikemux-relay.service sikemux-relay-certificate.service sikemux-relay-certificate.timer; do
  cmp -s "$here/$unit" "/etc/systemd/system/$unit" || relay_changed=1
  install -m 644 "$here/$unit" "/etc/systemd/system/$unit"
done
systemctl daemon-reload
systemctl enable sikemux-relay >/dev/null
systemctl enable --now sikemux-relay-certificate.timer >/dev/null
if command -v ufw >/dev/null; then
  ufw allow 7842/udp comment 'sikemux relay: QUIC address discovery' >/dev/null
fi

# Caddy asks for the certificate when it first loads the site, which takes a few seconds.
for _ in $(seq 30); do
  /usr/local/lib/sikemux/relay-certificate 2>/dev/null && break
  sleep 2
done
if [ -f /etc/sikemux-relay/tls.crt ]; then
  if [ "$relay_changed" = 1 ]; then systemctl restart sikemux-relay; else systemctl start sikemux-relay; fi
  sleep 2
  /usr/local/lib/sikemux/relay-health
else
  echo "the relay waits for Caddy's certificate for relay.sikemux.com; it starts within an hour of" \
    "the DNS record pointing here, or run /usr/local/lib/sikemux/relay-certificate" >&2
fi

step "watch: checks every minute, posting problems to Discord's #alerts"
id sikemux-watch >/dev/null 2>&1 ||
  useradd --system --no-create-home --home-dir /nonexistent --shell /usr/sbin/nologin sikemux-watch
command -v vnstat >/dev/null ||
  DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends vnstat >/dev/null
systemctl enable --now vnstat >/dev/null
install -m 755 -o root -g root "$here/watch" /usr/local/lib/sikemux/watch
install -m 644 "$here/sikemux-watch.service" /etc/systemd/system/sikemux-watch.service
install -m 644 "$here/sikemux-watch.timer" /etc/systemd/system/sikemux-watch.timer
systemctl daemon-reload
if [ -f /etc/sikemux/watch.env ]; then
  systemctl enable --now sikemux-watch.timer >/dev/null
else
  echo "the watch waits for /etc/sikemux/watch.env holding DISCORD_WEBHOOK_URL, the #alerts webhook" >&2
fi

step "done"
echo "citadel is ready for the first deploy"
