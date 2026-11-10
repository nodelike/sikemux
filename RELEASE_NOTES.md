# Sikemux v0.6.1-nightly.10

Nightlies are signed and delivered exactly like stable releases, but they carry unreleased work and can break. Switch back to stable in Settings → About whenever you want; you keep the build you are on until a stable release passes it.

## Before you update

- **Your phone needs 0.2.0-nightly.13 or later**, from TestFlight on iPhone or the Google Play testing track on Android.
- **This nightly links only with hosts on the 0.6.1 nightlies.** Hosts on stable 0.6.0 can't reach it.

## New since nightly.8

Nightly.9 was never published, so this nightly carries everything since nightly.8.

- **Hosts.** A Hosts page shows every computer on your account that runs Sikemux, with CPU, memory and disk over time and what each agent uses. A host in trouble says why under its name in the rail, raises an alert in Goals and on the notch, and offers to move work to a host with room. Open it with ⌘⇧H.
- **Smart accounts, on by default.** With two or more signed-in accounts for an agent, a new chat starts on the one with real headroom whose week ends soonest, and a project can keep to some accounts.
- **Snooze and settle chats** from a chat's menu, on the Mac and the phone. Snoozed and settled chats fold under their own groups in the rail, and idle agents sort by when you last wrote to them.
- **Mermaid diagrams.** A `mermaid` code block in a reply is drawn as its diagram, in the theme's colours.
- **Snip mode.** On a desk page, ⌘⇧X or the crosshair button drags out boxes that go straight into the front chat's draft as pictures.
- **Pages in a reply open in a tab**, and a page past the height cap says so.
- **Pin changes on a picture, a recording or a diff.** What you write on a slice becomes a numbered change list sent to the agent whose work it is.
- **Goal reports and drift audits** on a schedule you pick.
- **One project per repository across hosts**, and Copy to brings uncommitted changes as a commit.
- **Pull requests in the Git pane**, as its fourth page.
- **Held actions show their exact command** with Refuse, Allow once and Always allow, in the chat, on the phone and on the notch.

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
- On Linux, showing a page in a reply no longer freezes the app.
- Agents' builds take turns on a host and share compiled crates, and an agent worktree's build output is removed once nothing needs it.
- An agent's plugin write that credits Claude is refused, as commits already were.
- A crash in WebKit while describing a failed tab script is gone.
- An agent too busy to name its version in time is no longer shown as broken.
- A shared checkout with nothing uncommitted can be moved, and git run outside any agent is never refused.
- The notch peeks an inbox item once, not on every update.
- Codex, Muse and OMP marks stay sharp at any size.
- Under XWayland the window follows the scale of the display it is on, live.

For the complete patch history, compare [`v0.6.1-nightly.8...v0.6.1-nightly.10`](https://github.com/nodelike/sikemux/compare/v0.6.1-nightly.8...v0.6.1-nightly.10).
