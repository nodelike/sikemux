# Sikemux v0.6.1-nightly.13

Nightlies are signed and delivered exactly like stable releases, but they carry unreleased work and can break. Switch back to stable in Settings → About whenever you want; you keep the build you are on until a stable release passes it.

## Before you update

- **Your phone needs 0.2.0-nightly.13 or later**, from TestFlight on iPhone or the Google Play testing track on Android. Phone 0.2.0-nightly.20 comes out alongside this nightly.
- **This nightly links only with hosts on the 0.6.1 nightlies.** Hosts on stable 0.6.0 can't reach it.

## New since nightly.12

- **Crash reports.** Published builds of the app, the core and the notch send crashes and hangs to Sentry so we can fix them. Builds you make yourself send nothing.
- **Pictured choices.** When an agent's question offers pictures, they show as tiles under its reply, and the question box above the composer shrinks to one line.
- **A new chat shows the account it will start on** before it starts, and an asleep chat takes a model or effort pick.
- **Auto's settings come down to six**, and the board's side panels float over it instead of taking a column.
- **A page shown in a desk tab gets a full-width preview card** in the chat.
- **⌘K slides** the agent away beside the desk, as ⌘J does.
- Waits on the same thing back to back share one row, and name the command or run they wait on.

## Fixes

- The Mac app comes back after it updates itself.
- Opening an image, a PDF or any other file the viewer draws no longer crashes the app.
- Picking Opus 5.5 (Claude's default row) starts Claude on it, not on the model your Claude settings name.
- The app reaches its linked hosts, and the rail and Devices agree on whether it has.
- A host renamed on its computer is renamed on the account too.
- A page too big for the core's connection no longer shows as an empty box.

For the complete patch history, compare [`v0.6.1-nightly.12...v0.6.1-nightly.13`](https://github.com/nodelike/sikemux/compare/v0.6.1-nightly.12...v0.6.1-nightly.13).
