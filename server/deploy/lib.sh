# shellcheck shell=bash
# Sourced by the deploy scripts from the folder they run in: /usr/local/lib/sikemux on citadel,
# where setup-citadel.sh installs it beside them.

# The API, its migrations and the deploy user all reach the database as the sikemux role,
# through the local socket.
# shellcheck disable=SC2034
SIKEMUX_DATABASE_URL='postgresql://sikemux@%2Fvar%2Frun%2Fpostgresql/sikemux'

# Points rclone's r2: remote at R2_BUCKET from the R2_* settings, with no config file on disk.
use_r2() {
  : "${R2_BUCKET:?R2_BUCKET is not set}"
  : "${R2_ACCESS_KEY_ID:?R2_ACCESS_KEY_ID is not set}"
  : "${R2_SECRET_ACCESS_KEY:?R2_SECRET_ACCESS_KEY is not set}"
  [ -n "${R2_ENDPOINT:-}" ] || : "${R2_ACCOUNT_ID:?R2_ACCOUNT_ID is not set}"

  export RCLONE_CONFIG_R2_TYPE=s3
  export RCLONE_CONFIG_R2_PROVIDER=Cloudflare
  export RCLONE_CONFIG_R2_ENDPOINT="${R2_ENDPOINT:-https://$R2_ACCOUNT_ID.r2.cloudflarestorage.com}"
  export RCLONE_CONFIG_R2_ACCESS_KEY_ID="$R2_ACCESS_KEY_ID"
  export RCLONE_CONFIG_R2_SECRET_ACCESS_KEY="$R2_SECRET_ACCESS_KEY"
  export RCLONE_CONFIG_R2_NO_CHECK_BUCKET=true
}

rclone() { command rclone --config "" --use-server-modtime "$@"; }

# What the files `setup-citadel.sh` installs from this folder hash to, as pack.mjs writes it.
deploy_hash() {
  (cd "$1" && find . -maxdepth 1 -type f ! -name pack.mjs -printf '%P\n' | LC_ALL=C sort | xargs sha256sum | sha256sum | cut -d' ' -f1)
}
