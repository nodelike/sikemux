# How Sikemux fits together

A map of the codebase for someone opening it for the first time. The
[README](../../README.md) covers what the app does. [CONTRIBUTING](../../CONTRIBUTING.md)
covers setup and the checks you run before pushing.
[ADR 0001](./0001-keep-react-tauri.md) explains why the app stays on React and Tauri.

## Processes

Sikemux is a [Tauri 2](https://tauri.app) app. Tauri runs a native Rust process that
owns a window. Inside that window, a system web view (WKWebView on macOS) runs the
React interface. The two talk over IPC: the page calls named Rust commands, and Rust
sends events and byte streams back.

| Process           | Where it lives                                        | What it does                                                                                                                                                                                                                          |
| ----------------- | ----------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Main process      | `src-tauri/src/main.rs` → `lib.rs::run()`             | Owns the window, Git calls, language servers, watchers and browser tabs, and connects to the background core for terminals and agents. `lib.rs` registers the managed state and every IPC command.                                    |
| Background core   | `sikemux core` → `crates/sikemux-core`                | Owns every shell, terminal agent, task and chat agent, the harness and the agents' tool endpoint. It outlives the window; see [The background core](#the-background-core).                                                            |
| App web view      | `src/main.tsx`, window label `main`                   | The React UI. It loads `dist/` in builds and `http://localhost:1420` in dev. A guard in `lib.rs` stops it from ever navigating anywhere else.                                                                                         |
| Browser tabs      | `src-tauri/src/browser/`                              | Each tab is a native child web view placed over the main window. React draws only the tab chrome and reports where the page area is. `without_page_script.rs` keeps the app's own page scripts out of these tabs.                     |
| `sikemux-editor`  | `src-tauri/src/bin/sikemux-editor/` → `cli_client.rs` | The `sikemux` and `sikemux-editor` launchers, which ask the running app to open files and projects. With `--tools-mcp` it is the MCP server (Model Context Protocol) agents launch over stdio; with `core` it is the background core. |
| `sikemux-voice`   | `src-tauri/voice/` (Swift package)                    | Speech-to-text helper, downloaded with the speech model. `voice.rs` starts it, writes JSON lines to its stdin and relays its `voice` events.                                                                                          |
| Shells and agents | spawned by the background core                        | Children of the core, not of the app, so reloading, closing or quitting the app leaves them running.                                                                                                                                  |
| Language servers  | spawned by `lsp/`                                     | Children of the app. When the main web view reloads or the window closes, `lib.rs` drains them.                                                                                                                                       |

`sikemux-editor` ships as a Tauri sidecar, meaning an extra executable bundled next to
the app. `scripts/build-cli-sidecar.mjs` builds it into `src-tauri/binaries/` and
`tauri.sidecar.conf.json` adds it to the bundle. `scripts/build-voice-helper.mjs` builds
and signs `sikemux-voice` there too, but it stays out of the app: each release publishes
it as an asset, `build.rs` records its hash, and `voice_models.rs` downloads it with the
speech model. `pnpm sidecar:dev` builds both beside the dev app, which runs them in place.

### The background core

The app is a client of a long-lived background process, the core. It is the
`sikemux-editor` sidecar run as `sikemux core`, so it adds no executable to the bundle and
never loads WebKit. [ADR 0006](./0006-background-core.md) records why.

- **Starting.** The app starts the core on demand, detached in its own session, logging
  to `core.log` in the app's log directory. One core runs per user and channel; a lock
  file stops a second one. It exits by itself five minutes after it has no sessions and
  no client.
- **Talking.** A Unix socket, `~/.config/sikemux/core.sock` (`core.dev.sock` in debug
  builds, `SIKEMUX_CORE_SOCKET` to override), mode 0600. Frames are a length, a kind byte
  and a payload: control messages are JSON, terminal input and output are raw bytes.
  Every connection starts with a versioned handshake.
- **What it owns.** Terminals and terminal agents (`server/session.rs`, built on
  `sikemux-pty`), their screens and agent activity (`server/agent.rs`), tasks and their
  output logs, chat agents (`server/chat/`), harness runs, idempotency keys and the
  per-project event journal (`server/harness.rs`), and the agents' tool endpoint
  (`server/tools.rs`).
- **The app's side.** `src-tauri/src/pty/` keeps the same Tauri commands the page has
  always called and forwards them; core events are re-emitted as the same page events.
  `src-tauri/src/acp/` prepares a chat agent's launch and hands it to the core.
- **Taking sessions back.** A pane saves the id of the core session it shows. When the
  page loads, each pane takes its session back with its screen, or starts a fresh shell
  if the core no longer has it. Chats attach and replay what they missed. Sessions that
  nothing in the saved layout names are stopped after 30 seconds.
- **Exits.** Reloading, closing the window, Quit (⌘Q) and an update restart only
  disconnect. Quit and Stop Everything (⌥⌘Q) stops every session and the core. Closing a
  pane, stopping a task or closing an agent still stops that one process.
- **Updates.** A newer app finds an older core by its build identity and asks it to
  update. The core writes its state to a private directory, keeps its sockets and
  terminal file descriptors open across `exec`, and becomes the new binary in the same
  process, so every shell stays its child. A chat turn in progress delays the update by
  up to two minutes; chats are then restarted on their saved conversation.
- **Crashes.** A terminal agent that dies unexpectedly comes back on its conversation in
  the same pane (`src/agents/tuiRecovery.ts`); a chat agent does the same
  (`src/chat/sessionRecovery.ts`).
- **Paired devices.** With remote access on (Settings, Devices), the core also listens on
  an iroh endpoint, and phones that paired with a code reach it from anywhere
  (`server/remote.rs`, `server/pairing.rs`, `src/pairing.rs`). Each request is checked
  against what the device may do (`server/access.rs`). The app publishes its projects and
  how it starts each agent (`server/workspace.rs`, `src/shell/RemoteWorkspaceBridge.tsx`),
  so a device can start a chat with the window closed. [ADR 0007](./0007-remote-access-over-iroh.md)
  records the design.

### The CLI broker

The background core (`sikemux core`, in `crates/sikemux-core`) listens on a local TCP
port guarded by a random token, and writes the port and token to
`~/.config/sikemux/cli.json` (`cli.dev.json` in debug builds), removing it when it
exits. Both the `sikemux` launcher and the tools MCP server find it through that file,
or through `SIKEMUX_CLI_ENDPOINT`, which every terminal Sikemux starts receives. The app
registers with the core as the window; calls that need it, such as a file open, are
sent to the app over the core socket. File-open requests reach the UI as a
`cli-open-available` event, handled by `src/shell/CliOpenBridge.tsx`. Agent tool calls
go through the harness, described under [Agents](#agents).

## Frontend (`src/`)

| Folder                               | Owns                                                                                            |
| ------------------------------------ | ----------------------------------------------------------------------------------------------- |
| `state/`                             | The store, commands, persistence and the resource cache (see below)                             |
| `api/`                               | One typed wrapper module per backend area, plus the IPC transport                               |
| `workspace/`                         | The stage: `Workspace.tsx`, the tab bar, the agent desk and the search pane                     |
| `terminal/`                          | xterm.js panes and the PTY client (`usePty.ts`)                                                 |
| `editor/`                            | The CodeMirror editor pane and its find bar, insights and image viewer                          |
| `git/`                               | The Git pane, diffs, commit review and the graph                                                |
| `chat/`                              | The chat view for agents on ACP                                                                 |
| `agents/`                            | Agent launching, the agent picker, lifecycle and saved-session sync                             |
| `harness/`                           | The window half of the agent harness: inspecting, launching tasks and `ui.open`                 |
| `rail/`                              | Side rail, file tree and agent rail                                                             |
| `palettes/`                          | Command, file and new-tab palettes and the session switcher                                     |
| `settings/`                          | The settings page                                                                               |
| `shell/`                             | App-level bridges and overlays: CLI opens, harness requests, dialogs, toasts                    |
| `actions/`                           | The action registry that palettes and shortcuts run, scoped to global, project, session or item |
| `commands/`                          | Keymap, keybinding overrides and custom commands                                                |
| `workbench/`                         | Per-item controllers and their runtime, navigation history, project diagnostics                 |
| `projects/`                          | Project locations (local or SSH) and the per-project `sikemux.json` config                      |
| `tasks/`                             | Running the tasks a project declares                                                            |
| `extensions/`                        | Internal registry for contributed actions, workbench items and task providers                   |
| `plugin-api/`                        | The only code plugins may import                                                                |
| `plugins/`                           | The plugin registry and the six built-in plugins                                                |
| `codehost/`                          | Shared GitHub and Bitbucket UI, handed to those plugins through `plugin-api/codehost.ts`        |
| `themes/`                            | Built-in and Ghostty themes and the theme bus that writes colours onto `:root`                  |
| `styles/`                            | Global and shared CSS (see [Styling](#styling))                                                 |
| `ui/`, `hooks/`, `lib/`              | Shared components, React hooks, and utilities including telemetry                               |
| `markdown/`, `languages/`, `vendor/` | Markdown rendering, the generated grammar list, and Shiki wrappers                              |
| `voice/`, `test/`                    | The dictation client; Vitest setup and fixtures                                                 |
| `browser/`                           | Tests for the scripts injected into browser tabs (the scripts live in `src-tauri/src/browser/`) |

### State

There is one [Zustand](https://github.com/pmndrs/zustand) store, `useStore` in
`src/state/store.ts`. Its type is `DomainState & ViewState`: what the user built, and
how it is currently shown. Updates go through `mutate(draft => …)`, which uses Immer
so code can write to a draft instead of copying objects by hand.

Components read the store through selectors (`selectors.ts`) and change it by calling
functions from `src/state/commands/`, imported as `import * as cmd from "./state/commands"`.
That folder is the action layer: if something changes sessions, windows, panes, tabs,
agents or settings, it belongs there, not in a component. `shared.ts` holds helpers such
as `patchWindow` and `withActiveSession`. `bus.ts` is a small in-app event bus for
one-off signals such as `open-file`, `fs-changed` and `pane-closed`.

The objects nest like this (types in `src/state/types/domain.ts`):

```
Session            kind: project | command | ssh | <plugin kind>
└─ Window[]        one tab in the session's strip, with a role: term, files, git, agent, …
   └─ LayoutNode   a tree of SplitNode (row | column | stack) and PaneNode
      └─ PaneNode  kind: terminal | editor | git | diff | search | agent | desk | <plugin kind>
```

A project is a session of kind `project` whose `cwd` is the project root.
`projectRoots` lists the folders Sikemux scans to find projects. Agents live in their
own `agents` record. An agent's window has one pane whose id is the agent's id
(`agentWindow.ts`), so `agents[pane.id]` is its record. Each agent can also have a desk
(`desks`), which holds its browser tabs, the files it opened and its task terminals.

**Persistence.** `persist.ts` watches the store. Shortly after the last change it
serializes a snapshot and sends it through `state_save`. `src-tauri/src/state.rs` writes
it to SQLite at `~/.config/sikemux/state.sqlite3` (`state.dev.sqlite3` in debug, so dev
builds never touch installed state). Loading runs `applyHydrate`, which migrates older
snapshots and validates them with `persistValidation.ts`. The schema version is
`VERSION` in `persist.ts` and must match `APPLICATION_STATE_VERSION` in `state.rs`.

**Resources.** Backend data that is fetched rather than owned, such as Git overviews,
remotes, agent sessions and SSH hosts, goes through `resources.ts`. A definition in
`resources.defs.ts` names a `kind`, a `fetch` function and a `staleAfterMs`. Components
call `useResource(def, ...args)`. The cache shares one request between callers, is
bounded in size, and drops entries nothing has used for a while. `invalidate(predicate)`
refetches whatever matches, which is how file-watcher events refresh the UI.

## IPC

Every module in `src/api/` calls `invoke` from `src/api/invoke.ts`, which records
timing and error counts and supports cancellation through an `AbortSignal`. It
sits on `src/api/transport.ts`. Tests replace that transport with an in-memory one
through `installIpcTransportForTests`, so no test needs a real backend.

**The command list is generated.** Rust commands are registered once, in the
`tauri::generate_handler![…]` list in `src-tauri/src/lib.rs`.
`pnpm ipc:generate` (`scripts/generate-ipc-contracts.mjs`) reads that list and writes:

- `src-tauri/src/generated_command_names.rs`, which `build.rs` uses to generate a
  permission for each command;
- `src/api/generated/ipcCommands.ts`, the same names as a TypeScript type;
- `src-tauri/capabilities/default.json`, which allows exactly those commands for the
  `main` web view and nothing else. Browser tabs get no commands.

After adding or removing a command, run `pnpm ipc:generate` and commit the three files.
`pnpm ipc:check` fails if they are stale.

**Data coming back** uses two mechanisms:

- **Channels** carry streams to one caller. Terminal output is the busiest: the core
  keeps each PTY's (pseudo-terminal, the OS device a shell writes to) screen with `vt100`,
  and `pty_attach` returns that screen plus a channel of raw bytes the app relays from the
  core. The page reports progress with `pty_ack`, the app passes it on, and the core pauses
  reading a shell once too much output is unacknowledged. A hidden pane unsubscribes; its
  shell keeps running. Plugin streams and update downloads also use channels.
- **Events** are broadcast to the `main` web view with `emit_to`. Examples:
  `git_changed` from `fs_watch.rs` (the UI invalidates resources and fires `fs-changed`,
  see `subscribeGitChanged` in `App.tsx`), `lsp_diagnostics`, `acp_event`,
  `pty_shell_metadata`, `browser-tabs-changed`, `harness-request` and `voice`.

## Rust core (`src-tauri/src/`)

| Module                                        | Owns                                                                                                         |
| --------------------------------------------- | ------------------------------------------------------------------------------------------------------------ |
| `pty/`                                        | The connection to the background core, and the PTY commands that forward to it and relay its output          |
| `git/`, `diff.rs`                             | Status, log, branches, stash, worktrees, blame, commits and AI commit messages, mostly through `git2`        |
| `fs_watch.rs`                                 | One watcher per repo that emits `git_changed`                                                                |
| `files.rs`, `fs.rs`                           | Cached project file lists for the file palette, and directory listings for the tree                          |
| `search.rs`                                   | Project-wide search on ripgrep's engine                                                                      |
| `lsp/`                                        | Language server discovery, processes, JSON-RPC over stdio, documents and diagnostics                         |
| `acp/`                                        | Preparing a chat agent's launch and forwarding to the core (see [Agents](#agents))                           |
| `agents/`                                     | Finding agent executables, reading their saved sessions, models and usage                                    |
| `browser/`                                    | Browser tabs, the scripts injected into them, the agent browser tools, MCP wiring per agent (`agents.rs`)    |
| `harness.rs`                                  | Answering the tool calls the core hands to the window, and the queue of those the UI must answer             |
| `cli_*`                                       | The CLI client, `open` requests the core hands over, the launcher's paths and install                        |
| `plugins/`                                    | The plugin host and the compiled-in plugin list                                                              |
| `state.rs`                                    | Saving and loading the app state in SQLite                                                                   |
| `observability/`                              | Bounded spans, counters and latency history, the UI heartbeat and hang watchdog; `autopsy.rs` saves evidence |
| `system.rs`                                   | Login-shell `PATH`, the file descriptor limit, finding executables                                           |
| `updates.rs`                                  | Stable and nightly update checks and installs; `release_credits.rs` reads release notes                      |
| `voice.rs`                                    | The voice helper process                                                                                     |
| `settings.rs`, `ssh.rs`                       | Project root scanning and `~/.ssh/config` hosts                                                              |
| `transparency.rs`, `wallpaper.rs`, `wheel.rs` | macOS window blur, wallpaper stills and trackpad state                                                       |

Internal crates in `src-tauri/crates/`:

| Crate                | Purpose                                                                                                                                                                                                                                                                                                     |
| -------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `sikemux-plugin-api` | The contract between the app and a plugin: a method name and JSON in, JSON or a stream out                                                                                                                                                                                                                  |
| `sikemux-process`    | Runs a subprocess with a timeout and an output size limit                                                                                                                                                                                                                                                   |
| `sikemux-markdown`   | Parses markdown into the block tree the chat transcript draws (`markdown_parse`)                                                                                                                                                                                                                            |
| `sikemux-keychain`   | Reads and writes secrets in the system keychain; used by the GitHub, Bitbucket and SigNoz plugins                                                                                                                                                                                                           |
| `sikemux-pty`        | The terminal engine without Tauri: the per-PTY screen, shell integration, the configured shell and login-shell environment, the `SIKEMUX_*` environment, task checks, task output paging, and `agent_detection/`, which reads an agent's screen against `manifests/*.json` to tell working, blocked or idle |
| `sikemux-core`       | The background core (`sikemux core`): its Unix-socket server, wire protocol and client; terminals, tasks and chat agents; the harness, journal and tool endpoint; updating in place                                                                                                                         |

## Plugins

Plugins are compiled into the app. There is no marketplace and no runtime loading.
The six built-ins are AWS, Bitbucket, Bruno, GitHub, Rundeck and SigNoz. Each has two
halves:

- **Rust:** a crate in `src-tauri/plugins/<name>/` that implements the `Plugin` trait
  from `sikemux-plugin-api`. `src-tauri/Cargo.toml` gives each one a Cargo feature of
  the same name, all on by default, and `src-tauri/src/plugins/builtin.rs` lists them.
  Plugin crates opt in to the workspace Clippy lints that forbid `unwrap`, `expect`,
  `panic` and unchecked indexing, because a panicking plugin takes the whole app down.
- **TypeScript:** a folder in `src/plugins/<name>/` that calls `registerFrontendPlugin`.
  `src/plugins/builtin.ts` imports all six and is the only core file allowed to.

A plugin's frontend imports only `src/plugin-api`, its own folder, and React or
Zustand. `scripts/check-plugin-boundaries.mjs` (part of `pnpm lint`) enforces both
directions. When a plugin needs something from the app, add it to `src/plugin-api`.
Frontend calls reach Rust through `createPluginBackend`, which uses `plugin_call` and
`plugin_stream_start`. A plugin can also offer agent tools, which the MCP server lists
next to the built-in ones. Settings can switch a plugin off (`disabledPlugins`) without
removing it from the build.

## Agents

An agent runs on one of two transports.

- **Terminal.** The agent's own CLI runs in a PTY like any shell (`sikemux-pty`'s `launch.rs`).
  Sikemux tells its state from the screen (`sikemux-pty`'s `agent_detection/`) and finds its saved
  sessions on disk (`agents/sessions/<agent>.rs`). Every agent type supports this.
- **Chat (ACP).** [ACP](https://agentclientprotocol.com), the Agent Client Protocol, is
  JSON-RPC over stdio between an editor and an agent. Chat agents run in the background
  core, so a turn keeps going through a reload or a quit. `src-tauri/src/acp/mod.rs`
  prepares the launch (the adapter, the agent binary, the browser tools, the environment)
  and forwards prompts and permission answers to the core; `sikemux-core`'s
  `server/chat/` runs the session and keeps what it said, so a page that attaches later
  replays it. Claude and Codex are reached through adapter packages Sikemux installs.
  OpenCode, OMP, Grok and Hermes speak ACP themselves (`sikemux-core`'s `acp/native.rs`).
  `acp/air.rs` adds the Claude adapter's background task and subagent updates. On the UI
  side, `src/chat/useAcpSession.ts` and `reducer.ts` turn events into a transcript.
  `CHAT_AGENT_TYPES` in `src/agents/agentLaunch.ts` lists the chat-capable agents, and
  `src/chat/AgentSurface.tsx` switches one agent between chat and terminal views.

**Harness tools.** Agents on both transports get Sikemux's tools through
`sikemux-editor --tools-mcp`. `src-tauri/src/browser/agents.rs` knows how to register it with
each agent host. Tools are declared once in `browser/tools.json`, one-line descriptions
only. The protocol details live in `browser/SIKEMUX_GUIDE.md`, which agents fetch with
the `guide` tool. The MCP binary compiles both files in. A tool call travels:

```
agent → sikemux-editor --tools-mcp → the core's tool endpoint
  task.read / task.stop / events.wait     → answered by the core (server/harness.rs)
  task.start / task.restart               → the core keeps the run, the window launches it
  everything else                         → the window: harness.rs in the app
    browser.*                             → browser/tools.rs (Rust)
    plugins.tools / plugins.call          → plugins/agent.rs (Rust)
    the rest                              → harness-request event → HarnessBridge →
                                            src/harness/service.ts → harness_reply
```

The core keeps runs, idempotency keys and an event journal per project
(`journal/<hash>.jsonl` in the app's data directory), so task tools work with the
window closed and event cursors survive restarts.

`pnpm agent-tools:generate` writes `src-tauri/crates/sikemux-core/src/cli/methods.rs` and checks
that every declared method has a handler in `browser/tools.rs`, the core's
harness dispatch or `src/harness/service.ts`, and that no handler lacks a declaration. The README's
[Agent harness tools](../../README.md#agent-harness-tools) section describes the tools
from the user's side.

## Styling

`src/styles.css` imports the sheets every screen needs, in cascade order. Two matter
most:

- `src/styles/tokens.css` defines type, spacing, radius, elevation and the colour
  ramp. All of it derives from four values the theme bus writes on `:root`, so a new
  theme is one entry in `src/themes/index.ts`.
- `src/styles/modern-shell.css` is the shell's layout and surfaces. It gives every
  `<button>` the `--radius-2` corner.

Large feature sheets are split into small files behind an `@import` file that
bundles them. Add a new file to the bundling file, not to `styles.css`:

| Bundling file                     | Parts                         | Loaded                                |
| --------------------------------- | ----------------------------- | ------------------------------------- |
| `src/styles/git.css`              | `src/styles/git/`             | at startup                            |
| `src/styles/chat.css`             | `src/styles/chat/`            | by `chat/AgentSurface.tsx`            |
| `src/styles/settings.css`         | `src/styles/settings/`        | by `settings/SettingsPanel.tsx`       |
| `src/codehost/codehost.css`       | `src/codehost/styles/`        | by `codehost/components/HostArea.tsx` |
| `src/plugins/rundeck/rundeck.css` | `src/plugins/rundeck/styles/` | by the Rundeck components             |

Read [DESIGN.md](../../DESIGN.md) before changing anything visual, and the UI rules in
[AGENTS.md](../../AGENTS.md).

## Mobile app (`mobile/`)

The phone app pairs with a Mac's core and drives it over iroh (see
[The background core](#the-background-core)). It is an Expo app (`mobile/app`) that calls
the core's own client through `@sikemux/native` (`mobile/native`), a Turbo Module that
`uniffi-bindgen-react-native` generates from `src-tauri/crates/sikemux-mobile`. That crate
wraps `sikemux-core`'s `client`, `pairing` and `remote` modules: requests and answers cross
as the core's protocol JSON, terminal bytes as bytes, and the device's key stays in the
phone's secure store. `mobile/` is a pnpm workspace of its own so the desktop build never
installs it. [ADR 0008](./0008-mobile-app-expo-and-rust-client.md) records the choices.

## Quality ratchets

These limits only move in one direction. All of them run in `pnpm check` and the
pre-push hook.

| Check              | Where                                                                                | How to move it                                                                                                        |
| ------------------ | ------------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------- |
| Test coverage      | `test.coverage.thresholds` in `vite.config.ts`                                       | Raise the numbers after `pnpm test:coverage` shows coverage went up. Never lower them.                                |
| `!important` count | `BUDGET` in `scripts/check-css-important.mjs`                                        | Lower it when you remove one. Only goes down.                                                                         |
| Bundle size        | `scripts/check-performance-budget.mjs`, run by `pnpm perf:budget` after `pnpm build` | Every ceiling needs 10% headroom. Prefer lazy imports; raise a ceiling only with measured output and a stated reason. |
| Plugin boundaries  | `scripts/check-plugin-boundaries.mjs`                                                | Not a number: add what a plugin needs to `src/plugin-api`.                                                            |
| Generated files    | `pnpm ipc:check`, `pnpm agent-tools:check`, `pnpm grammars:check`                    | Run the matching `:generate` script and commit its output.                                                            |
