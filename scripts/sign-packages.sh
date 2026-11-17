#!/usr/bin/env bash
# Writes the signed indexes a release's apt repository serves, packed as the one
# archive the release carries:
#   apt/<suite>/xz/  Packages.xz with the InRelease, Release and Release.gpg that name
#                    it, for each suite this version may serve: nightly, and stable too
#                    for a stable version, since apt refuses a Release of another suite
#   apt/<suite>/gz/  the same index as Packages.gz, so a suite can move with no moment
#                    where its index and the signed file naming it disagree
# The key comes in PACKAGES_SIGNING_KEY (an armored signing subkey) and
# PACKAGES_SIGNING_KEY_PASSWORD; it never reaches a job that builds.
#   scripts/sign-packages.sh <version> <bundle dir>
set -euo pipefail

cd "$(dirname "$0")/.."
VERSION="${1:?usage: $0 <version> <bundle dir>}"
BUNDLE="${2:?usage: $0 <version> <bundle dir>}"
OUT=target/packages
INDEX="$OUT/packages-index-$VERSION.tar"

fail() {
  echo "Signing packages failed: $*" >&2
  exit 1
}

: "${PACKAGES_SIGNING_KEY:?PACKAGES_SIGNING_KEY is not set}"
: "${PACKAGES_SIGNING_KEY_PASSWORD:?PACKAGES_SIGNING_KEY_PASSWORD is not set}"
for command in gpg gpgv apt-ftparchive xz gzip; do
  command -v "$command" >/dev/null || fail "$command is not installed"
done
shopt -s nullglob
DEBS=("$BUNDLE"/deb/*.deb)
[[ ${#DEBS[@]} -gt 0 ]] || fail "no .deb in $BUNDLE/deb"

WORK="$(mktemp -d)"
export GNUPGHOME="$WORK/gnupg"
trap 'gpgconf --kill all >/dev/null 2>&1 || true; rm -rf "$WORK"' EXIT
mkdir -m 700 "$GNUPGHOME"
PASSPHRASE="$GNUPGHOME/passphrase"
printf '%s' "$PACKAGES_SIGNING_KEY_PASSWORD" >"$PASSPHRASE"
gpg --batch --quiet --import <<<"$PACKAGES_SIGNING_KEY" 2>/dev/null || fail "the signing key does not import"
KEY="$(gpg --batch --list-secret-keys --with-colons | awk -F: '$1 == "fpr" { print $10; exit }')"
[[ -n "$KEY" ]] || fail "the signing key holds no secret key"
gpg --batch --export "$KEY" >"$WORK/public.gpg"

sign() {
  gpg --batch --yes --quiet --pinentry-mode loopback --passphrase-file "$PASSPHRASE" \
    --local-user "$KEY" --digest-algo SHA512 "$@"
}

TREE="$WORK/index"
mkdir -p "$TREE/apt" "$OUT"

echo "→ Writing the apt index"
# A flat repository names each file from the address its sources line gives,
# which is the folder of every release, so a package is fetched from its own.
mkdir -p "$WORK/apt/v$VERSION"
cp "${DEBS[@]}" "$WORK/apt/v$VERSION/"
(cd "$WORK/apt" && apt-ftparchive packages "v$VERSION") >"$WORK/Packages"
[[ "$(grep -c '^Filename: ' "$WORK/Packages")" -eq ${#DEBS[@]} ]] || fail "the apt index does not list every .deb"
SUITES=(nightly)
[[ "$VERSION" == *-* ]] || SUITES=(stable nightly)
for suite in "${SUITES[@]}"; do
  for format in xz gz; do
    apt="$TREE/apt/$suite/$format"
    mkdir -p "$apt"
    case "$format" in
      xz) xz -c "$WORK/Packages" >"$apt/Packages.xz" ;;
      gz) gzip -9nc "$WORK/Packages" >"$apt/Packages.gz" ;;
    esac
    # apt skips an index whose Release does not list it uncompressed too.
    cp "$WORK/Packages" "$apt/Packages"
    apt-ftparchive \
      -o APT::FTPArchive::Release::Origin=Sikemux \
      -o APT::FTPArchive::Release::Label=Sikemux \
      -o APT::FTPArchive::Release::Suite="packages-$suite" \
      -o APT::FTPArchive::Release::Description="Sikemux $VERSION" \
      release "$apt" >"$WORK/Release"
    rm "$apt/Packages"
    mv "$WORK/Release" "$apt/Release"
    sign --clearsign --output "$apt/InRelease" "$apt/Release"
    sign --armor --detach-sign --output "$apt/Release.gpg" "$apt/Release"
    gpgv --quiet --keyring "$WORK/public.gpg" "$apt/InRelease" 2>/dev/null ||
      fail "the $suite $format InRelease does not check against the key"
  done
done

tar --owner=0 --group=0 --numeric-owner --sort=name -cf "$INDEX" -C "$TREE" apt
echo "✓ wrote $INDEX"
