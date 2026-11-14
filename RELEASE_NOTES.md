# Sikemux v0.6.1-nightly.20

Nightlies are signed and delivered exactly like stable releases, but they carry unreleased work and can break. Switch back to stable in Settings → About whenever you want; you keep the build you are on until a stable release passes it.

## Before you update

- **Your phone needs 0.2.0-nightly.13 or later**, from TestFlight on iPhone or the Google Play testing track on Android.
- **This nightly links only with hosts on the 0.6.1 nightlies.** Hosts on stable 0.6.0 can't reach it.

## New since nightly.19

- **A chat's composer no longer answers a call.** Only the card's options and its field do; what you type in the composer reaches the agent as a message and leaves the call open.
- **Return in a call card's field sends the answer**, on the Mac and in the phones' secret fields.
- **A blocked terminal agent shows "needs you"**, such as one reporting state=blocked over OSC 7501 or showing a permission prompt.
- **Mods keep their data and files** in your account, sealed so the server can't read them, on every host. Agents read and write them with `mod_read` and `mod_write`, with Undo in the mod's tab.
- **Chief's workers** hang one level under the chief in the rail, and settled agents no longer show as a group on the phones.

## Fixes

- `git_commit`'s push no longer asks to change Stable releases.
- Builds through kache no longer fill the disk with its incremental copies.
- A land held or sent back by the merge queue always reaches its agent, with the failing log saved.
- Git that a build or its tests run is no longer checked as an agent's own, so the queue's tests stop failing on a busy machine.
- Running subagents spin again, ⌘N brings the new chat forward at once, and a rate-limited Claude account keeps its last plan limits.

For the complete patch history, compare [`v0.6.1-nightly.19...v0.6.1-nightly.20`](https://github.com/nodelike/sikemux/compare/v0.6.1-nightly.19...v0.6.1-nightly.20).
