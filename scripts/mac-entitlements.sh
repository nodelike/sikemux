#!/usr/bin/env bash
# Print the entitlements a release signs the app or its core with: every
# build's own, plus the keychain group the two share, which they may claim
# only because the provisioning profiles in packaging/macos allow it.
set -euo pipefail

cd "$(dirname "$0")/.."
TEAM=D577WD6Z5U
APP_IDENTIFIER="$(node scripts/app-metadata.mjs identifier)"
case "${1:-}" in
  app) APPLICATION="$TEAM.$APP_IDENTIFIER" ;;
  core) APPLICATION="$TEAM.$APP_IDENTIFIER.core" ;;
  *) echo "Usage: $0 app|core" >&2; exit 2 ;;
esac

PLIST="$(mktemp)"
trap 'rm -f "$PLIST"' EXIT
cp packaging/Entitlements.plist "$PLIST"
/usr/libexec/PlistBuddy \
  -c "Add :com.apple.application-identifier string $APPLICATION" \
  -c "Add :com.apple.developer.team-identifier string $TEAM" \
  -c "Add :keychain-access-groups array" \
  -c "Add :keychain-access-groups:0 string $TEAM.$APP_IDENTIFIER" \
  "$PLIST"
/usr/bin/plutil -convert xml1 -o - "$PLIST"
