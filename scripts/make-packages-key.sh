#!/usr/bin/env bash
# Makes the OpenPGP key that signs Sikemux's apt and rpm repositories, once, on the
# owner's Mac. The primary key and its revocation certificate stay in
# ~/.config/sikemux/release for the owner to back up, and the passphrase goes in the
# Keychain. Only the signing subkey, which expires, goes to the release environment.
# Writes the public half to packaging/linux/sikemux-archive.asc. Prints nothing secret.
#   scripts/make-packages-key.sh
set -euo pipefail

cd "$(dirname "$0")/.."
DIR="$HOME/.config/sikemux/release"
SERVICE="Sikemux packages signing key"
PRIMARY="$DIR/packages-signing-primary.asc"
SUBKEY="$DIR/packages-signing-subkey.asc"
REVOCATION="$DIR/packages-signing-revocation.asc"
PUBLIC=packaging/linux/sikemux-archive.asc
NAME="Sikemux packages <contact@sikemux.com>"

fail() {
  echo "Making the packages key failed: $*" >&2
  exit 1
}

command -v gpg >/dev/null || fail "gpg is not installed (brew install gnupg)"
[[ ! -e "$PRIMARY" ]] || fail "$PRIMARY exists; a second key would leave every machine trusting the first"
! security find-generic-password -s "$SERVICE" >/dev/null 2>&1 || fail "the Keychain already holds \"$SERVICE\""

umask 077
mkdir -p "$DIR"
GNUPGHOME="$(mktemp -d)"
export GNUPGHOME
trap 'gpgconf --kill all >/dev/null 2>&1 || true; rm -rf "$GNUPGHOME"' EXIT
PASSPHRASE="$GNUPGHOME/passphrase"
openssl rand -base64 33 | tr -d '\n' >"$PASSPHRASE"

with_passphrase() {
  gpg --batch --quiet --pinentry-mode loopback --passphrase-file "$PASSPHRASE" "$@"
}

# RSA rather than Ed25519: RHEL 9 in FIPS mode and older rpm refuse EdDSA signatures.
with_passphrase --quick-generate-key "$NAME" rsa4096 cert never
KEY="$(gpg --batch --list-keys --with-colons | awk -F: '$1 == "fpr" { print $10; exit }')"
[[ -n "$KEY" ]] || fail "gpg made no key"
with_passphrase --quick-add-key "$KEY" rsa4096 sign 2y

with_passphrase --armor --export-secret-keys "$KEY" >"$PRIMARY"
with_passphrase --armor --export-secret-subkeys "$KEY" >"$SUBKEY"
cp "$GNUPGHOME/openpgp-revocs.d/$KEY.rev" "$REVOCATION"
gpg --batch --armor --export "$KEY" >"$PUBLIC"
chmod 644 "$PUBLIC"
security add-generic-password -a "$USER" -s "$SERVICE" -l "$SERVICE" -w "$(cat "$PASSPHRASE")"

cat <<DONE
✓ Made the packages key $KEY
  Public half:  $PUBLIC (commit it with the package side)
  Signing subkey, expires in 2 years:  $SUBKEY
  Primary key:  $PRIMARY
  Revocation certificate:  $REVOCATION
  Passphrase:  Keychain item "$SERVICE"

Back up the primary key and the revocation certificate away from this Mac.

Give the release environment the signing subkey:
  gh secret set PACKAGES_SIGNING_KEY --env release <"$SUBKEY"
  security find-generic-password -s "$SERVICE" -w | gh secret set PACKAGES_SIGNING_KEY_PASSWORD --env release
DONE
