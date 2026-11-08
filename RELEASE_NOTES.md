# Sikemux v0.6.1-nightly.8

Nightlies are signed and delivered exactly like stable releases, but they carry unreleased work and can break. Switch back to stable in Settings → About whenever you want; you keep the build you are on until a stable release passes it.

## Before you update

- **Your phone needs 0.2.0-nightly.13 or later**, from TestFlight on iPhone or the Google Play testing track on Android.
- **This nightly links only with hosts on the 0.6.1 nightlies.** Hosts on stable 0.6.0 can't reach it.

## New since nightly.7

- **Auto runs without you for longer.** It brings back agents whose turn failed or whose process died, holds a card that spends without progress, checks each change on the latest branch before merging, and takes cards agents add, up to 64 agents at once. When this host is full, it starts agents on linked hosts you name in Auto's settings.
- **Auto's reviewer uses the change.** It builds and runs the commit instead of only reading a diff, a builder agrees a contract before it builds, and you can turn on scaling review to the risk of the change.
- **Standing roles and a reservoir.** A project can name roles in `sikemux.json` that come back as work lands, such as docs or duplicate code, and a `reservoir` command that keeps Auto supplied with work.
- **Pi in chat.** Pi now runs as a chat, on the Mac and the phone, with its dialogs answered in the question card.
- **Connected keys stay out of agents.** A provider's key no longer enters an agent's environment, for Pi, OpenCode and OMP too. They reach it through the core's own model proxy.
- **An agent's first write to a website or connector waits for you**, as the docs promised of everything leaving this Mac.
- **A failed check reaches one agent**, the author of the commit that broke it, with the lines that name the error.
- **Follow Omarchy's background.** On Omarchy, panes follow its wallpaper unless you choose a picture yourself.

## Fixes

- A rule an agent wrote no longer reaches later agents as if it were yours.
- A teammate's own checkout no longer holds up tab switches and messages while a large build folder is copied.
- Under XWayland the app draws at the scale the compositor gives its displays.
- The board stays fast with thousands of cards.

For the complete patch history, compare [`v0.6.1-nightly.7...v0.6.1-nightly.8`](https://github.com/nodelike/sikemux/compare/v0.6.1-nightly.7...v0.6.1-nightly.8).
