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
- Quit (⌘Q) leaves everything running; Quit and Stop Everything (⌥⌘Q) stops it. People
  asked for both.
- A newer app updates an older core in place with `exec`, keeping file descriptors and
  the process id, so shells survive updates. Chat connections cannot be carried across,
  so the core waits for running turns and then restarts chats on their saved
  conversation.

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
- If the core itself dies, its shells die with it. Chats resume on their saved
  conversation; terminal agents show that they exited.

## Alternatives considered

- **A separate core executable.** Rejected: it grows the download more than reusing the
  sidecar.
- **Running the full app binary headless as the core.** Rejected: it would load WebKit's
  frameworks for a process that never draws.
- **Handing terminals to a new core process through a second process.** Rejected in
  favour of `exec`, which keeps the process id, so shells stay children of the core and
  their exit codes still arrive.
