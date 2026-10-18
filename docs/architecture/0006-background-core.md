# ADR 0006: Terminals and agents live in a background core

- Status: Accepted
- Decision date: 2026-10-01

## Context

Every shell, terminal agent, task and chat agent used to be a child of the app.
Reloading the page, closing the window, quitting and installing an update all killed
them, so an agent's turn, a dev server or a long build died with the window. Agents'
tool calls also went through the app, so they failed while it was closed.

Sikemux is meant to be a harness agents work in, not only a window people look at. Work
an agent started should not depend on the window staying open.

## Decision

- A long-lived background process, the core, owns terminals, terminal agents, tasks,
  chat agents, the harness state and the agents' tool endpoint. The app is a client of
  it over a Unix socket.
- The core is the existing `sikemux-editor` sidecar run as `sikemux core`. It needs no
  new executable in the bundle and never loads WebKit.
- The app keeps its Tauri commands and page events unchanged and forwards them, so the
  frontend did not have to be rewritten.
- How an agent launches (finding its program, its arguments, environment, account folder
  and hosted providers' keys) is built in one place in the core crate, which the app and
  the core both call. The core keeps the app's projects and the person's agent accounts
  in its data folder and never their environment, finds each agent's program itself and
  asks each agent's CLI whether it is signed in, so it can start a chat, or wake one the
  app put to sleep, with no app running. While the app's window is open, it still wakes
  its own chats.
- Quit (⌘Q) leaves everything running; Quit and Stop Everything (⌥⌘Q) stops it. People
  asked for both.
- A newer app updates an older core in place with `exec`, keeping file descriptors and
  the process id, so shells survive updates. Chat connections cannot be carried across,
  so the core waits for running turns and then restarts chats on their saved
  conversation.
- A chat's turn outlives the core. The core writes each chat down in its data folder as
  it changes (its launch without keys or the person's shell variables, its session, its
  agent's process and whether a turn is at work) and keeps its transcript on disk. A
  core that starts after one that died, or after the host restarted, takes every chat
  up again by itself: it stops what the dead core's agent left running, reads the
  transcript back, resumes the provider's saved session and, for a turn the restart cut
  short, asks the agent to check what got done and carry on. An agent that dies under
  its chat is started again the same way, until it has died three times in five minutes.
  We do not own Claude's or Codex's agent loop, so carrying on means reopening the
  session the provider saved, which holds every tool call that finished.
- Something has to start a core that died. While remote access is on, the login item
  does (launchd's `KeepAlive` on an unsuccessful exit, systemd's `Restart=on-failure`);
  while the app is open, it starts a new core as soon as its connection drops and
  attaches to the chats that core took up. With remote access off and the app closed,
  nothing starts it until the person opens Sikemux, and the turn carries on then;
  nobody can reach that host meanwhile anyway.

## Consequences

- Quitting no longer stops processes. The first launch that takes sessions back says so
  once and points at ⌥⌘Q.
- Terminal output crosses one more hop. A 100 MB `cat` measured 45–72 MB/s through the
  core; xterm in the page remains the slower stage.
- The sidecar grew from about 0.8 MB to about 5.3 MB, mostly the terminal engine, the
  async runtime and the ACP crate.
- The core is Unix-only. The app no longer builds for Windows.
- The upgrade request is a frozen message every core understands, and the hand-over file
  has a format number with explicit migrations, so cores from different builds can always
  hand over to each other.
- If the core itself dies, its shells die with it, and terminal agents show that they
  exited. Chats come back on their own, and a turn that was running carries on; a step
  the crash cut off mid-way may be done twice.
- The app still tells the core which projects are open and edits the accounts the core
  keeps, since the window is where people set those up. Provider keys stay in the
  Keychain on macOS and in files only the person can read elsewhere; the core reads them
  as it launches.

## Alternatives considered

- **A separate core executable.** Rejected: it grows the download more than reusing the
  sidecar.
- **Running the full app binary headless as the core.** Rejected: it would load WebKit's
  frameworks for a process that never draws.
- **Handing terminals to a new core process through a second process.** Rejected in
  favour of `exec`, which keeps the process id, so shells stay children of the core and
  their exit codes still arrive.
