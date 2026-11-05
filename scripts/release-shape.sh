#!/usr/bin/env bash
# What a macOS release holds and who signs it, for the scripts that build, sign
# and check one. Sourced; it runs nothing.
# shellcheck disable=SC2034

# Published beside the app, which downloads them when first used, so they stay out of it.
PUBLISHED_HELPERS=(sikemux-voice sikemux-sim)
# The README and sikemux.com promise a small download.
DMG_BUDGET_BYTES=30000000
TEAM=D577WD6Z5U
TAURI_CLI_VERSION=2.12.0
# No build sees these; each signing tool gets only the one it needs.
SIGNING_SECRETS=(TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD
  APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID APPLE_API_KEY APPLE_API_ISSUER APPLE_API_KEY_PATH)
