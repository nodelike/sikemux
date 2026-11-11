# Sikemux v0.6.1-nightly.15

Nightlies are signed and delivered exactly like stable releases, but they carry unreleased work and can break. Switch back to stable in Settings → About whenever you want; you keep the build you are on until a stable release passes it.

## Before you update

- **Your phone needs 0.2.0-nightly.13 or later**, from TestFlight on iPhone or the Google Play testing track on Android. Phone 0.2.0-nightly.21 comes out alongside this nightly, and on iPhone it is the new native app.
- **This nightly links only with hosts on the 0.6.1 nightlies.** Hosts on stable 0.6.0 can't reach it.

## New since nightly.14

- **Hosts and projects across hosts.** One repository is one project on every host, however its remote is written. Copies and moves between hosts never lose your work, and a host out of reach says why, step by step, on its Devices card.
- **The account vault.** A host on an account keeps a vault for your connectors' sign-ins, with a recovery key you save once, in Settings → Connectors.
- **Limits per account.** The rail lists each account instead of adding them up, and the accounts card shows each one's plan, limits and resets.
- **The composer's dock.** What runs for a chat sits in one dock under the composer's top edge, and a new chat's host is a picker.
- **Chat strips say what they did.** An opened strip names what a search did, shows the files it touched as chips and the pictures it looked at, and older thoughts fold behind "Earlier thinking".
- **A busy Mac says so.** When agents' builds and simulators overload this Mac, the title bar and the notch list them, with Stop on each. Agents' Xcode, Gradle and Swift builds take turns as cargo builds do.
- **`sikemux status --json`** lists the host's problems, worst first, with what fixes each.

## Fixes

- A host update that keeps crashing at start is rolled back.
- A host with two clones of a repository never picks whichever sorts first.
- A host renamed in Devices keeps its name on the account.
- Desk pages show their own cursors.

For the complete patch history, compare [`v0.6.1-nightly.14...v0.6.1-nightly.15`](https://github.com/nodelike/sikemux/compare/v0.6.1-nightly.14...v0.6.1-nightly.15).
