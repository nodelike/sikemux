# Sikemux v0.6.1-nightly.7

Nightlies are signed and delivered exactly like stable releases, but they carry unreleased work and can break. Switch back to stable in Settings → About whenever you want; you keep the build you are on until a stable release passes it.

## Before you update

- **Your phone needs 0.2.0-nightly.13 or later**, from TestFlight on iPhone or the Google Play testing track on Android.
- **This nightly links only with hosts on the 0.6.1 nightlies.** Hosts on stable 0.6.0 can't reach it.

## New since nightly.6

- **Home shows only your calls.** Each is a card leading with what to decide, then one plain line per goal. The head says how many calls are for you and when everything under way should be through.
- **A goal's page reads as an account** beside its slices, with its short name, its lead, and why its ETA slipped when it did.
- **You look once per goal**, and only at work that shows something. Slices go Built then Landed, and the goal's Looks right card shows every picture its slices took.
- **Agents decide what is a goal.** The offer under the composer is gone: a chat's own agent decides whether its work should be tracked.
- **Always allow.** A project's own ship command, or a held deploy that reads as no production, can be always allowed instead of allowed once each run.
- **Arch and Omarchy.** A native pacman package, `sikemux-bin` or `sikemux-nightly-bin`, and on Omarchy the Omarchy theme follows the system's.
- **`browser_act` plays a whole flow in one call**, with navigating, waiting and reading alongside clicking and typing.

## Fixes

- Only messages from other agents fold, not an agent's own answers.
- A choice an agent leaves you in its reply, or an ask that is your call, reaches Needs you.
- Needs work hands a slice back to its owner, and a slice waiting on another starts once that one is Built.
- A deploy-only slice can be closed by its owner rather than waiting on a commit.
- A chat moved to another project goes with its tab, and a chat the network cut short stays awake.
- The window's shortcuts work while a chat shows a question, and clicking in a message being edited keeps the caret there.
- A secret answer shows a dot for each character as you type.
- A parked browser tab no longer takes swipe snapshots, which could abort the app.
- A failed one-shot Claude answer says why, and an invalid or untrusted `sikemux.json` says so rather than reading as naming no check.

For the complete patch history, compare [`v0.6.1-nightly.6...v0.6.1-nightly.7`](https://github.com/nodelike/sikemux/compare/v0.6.1-nightly.6...v0.6.1-nightly.7).
