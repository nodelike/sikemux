# Sikemux v0.6.1-nightly.9

Nightlies are signed and delivered exactly like stable releases, but they carry unreleased work and can break. Switch back to stable in Settings → About whenever you want; you keep the build you are on until a stable release passes it.

## Before you update

- **Your phone needs 0.2.0-nightly.13 or later**, from TestFlight on iPhone or the Google Play testing track on Android.
- **This nightly links only with hosts on the 0.6.1 nightlies.** Hosts on stable 0.6.0 can't reach it.

## New since nightly.8

- **A new first-run setup.** Two required steps, Agents then Projects, on one poster-like screen. Every agent is listed with its state and the action it needs, installs and sign-ins run right there, and any one agent that works is enough to continue.
- **Ruler.** The Limits panel becomes Ruler: Claude and Codex side by side, each with its tightest window and countdown, and the week left summed across accounts.
- **Answered questions read as a card**, one per round, each question with your answer under it.
- **Code blocks are a raised card** with their language's icon, and wrap and copy buttons.
- **Closing a chat pauses its triggers** instead of deleting them. The toast offers Undo, and Recent keeps the chat with its paused triggers until you take it up again.
- **Boards tidy themselves.** A finished card moves to the archive after three days, a card whose cards under it are all done is marked done, and a card whose owner went quiet goes back to Open.
- **The desk's viewport menu** holds a page at Desktop, Tablet or Phone size, or Fit to follow the pane. ⌘K shows the desk alone and brings the agent back.
- **Open a folder by path** in ⌘O, starting with / or ~.
- **All projects lists agents on one line each.**
- **Linux on arm64.** The desktop app now ships for arm64 as well as x86_64, as a deb, rpm, AppImage and tarball.
- **Home's composer is the chat's**, with Stash, YOLO, the model, effort, the project, worktree and account in its bar.

## Fixes

- The bundled SQLite is updated for RUSTSEC-2026-0459.
- A crash in WebKit while describing a failed tab script is gone.
- An agent too busy to name its version in time is no longer shown as broken.
- A shared checkout with nothing uncommitted can be moved, and git run outside any agent is never refused.
- The notch peeks an inbox item once, not on every update.
- Codex, Muse and OMP marks stay sharp at any size.
- Under XWayland the window follows the scale of the display it is on, live.

For the complete patch history, compare [`v0.6.1-nightly.8...v0.6.1-nightly.9`](https://github.com/nodelike/sikemux/compare/v0.6.1-nightly.8...v0.6.1-nightly.9).
