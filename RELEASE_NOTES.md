# Sikemux v0.6.1-nightly.17

Nightlies are signed and delivered exactly like stable releases, but they carry unreleased work and can break. Switch back to stable in Settings → About whenever you want; you keep the build you are on until a stable release passes it.

## Before you update

- **Your phone needs 0.2.0-nightly.13 or later**, from TestFlight on iPhone or the Google Play testing track on Android.
- **This nightly links only with hosts on the 0.6.1 nightlies.** Hosts on stable 0.6.0 can't reach it.

## New since nightly.16

- **/clear and /yolo** join /model as commands Sikemux answers itself: /clear starts a fresh chat with the same agent, account, model and mode, and /yolo switches asking before acting.
- **Limits read as used**, as Claude's own usage page does, and the top bar's popover is the accounts card alone.
- **Files on a message show as small framed cards**, in the composer and the transcript alike.
- **⌘J and ⌘K step through the split**: chat alone, chat with its desk, and the desk alone are always one press apart.

## Fixes

- Nightly.16 crashed on open when a desk came back empty from the last run. It opens again.
- A chat's live plan limits go to the account it really runs on.
- Moving a chat to another account mid-turn no longer tells its lead it stopped.
- A linked host shows online within seconds of coming back.
- /model opens the composer's model picker instead of sending "/model" as text.
- A resumed Claude chat shows an answer with a pasted image as an answer card.

For the complete patch history, compare [`v0.6.1-nightly.16...v0.6.1-nightly.17`](https://github.com/nodelike/sikemux/compare/v0.6.1-nightly.16...v0.6.1-nightly.17).
