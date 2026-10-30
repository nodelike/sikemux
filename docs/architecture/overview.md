# How Sikemux fits together

A map of the codebase for someone opening it for the first time. The
[README](../../README.md) covers what the app does. [CONTRIBUTING](../../CONTRIBUTING.md)
covers setup and the checks you run before pushing.
[ADR 0020](./0020-gpui-desktop-and-web.md) explains why the desktop app is GPUI drawing what
the core sends.

## Words

One word for each idea, in the interface, docs and the core's messages.

| Word                      | Means                                                                                               |
| ------------------------- | --------------------------------------------------------------------------------------------------- |
| host                      | Anything running the core: a server, a Mac mini, a laptop. "This host", never "this computer".      |
| device                    | What the person holds to reach hosts: a phone, a laptop, a browser.                                 |
| app                       | The software on a device: the desktop app, phone app, web app and the notch.                        |
| pair                      | A device on another account joins a host, once the host's owner allows it.                          |
| link                      | Every host on one account works for the others ([ADR 0018](./0018-the-account-is-the-trust.md)).    |
| project                   | A folder on one host, listed under that host ([ADR 0019](./0019-projects-belong-to-their-host.md)). |
| you                       | Devices on the host's own account with full access, which act as the person. In code, "the person". |
| full access               | Can run any command on the host as the person.                                                      |
| watch                     | Can read, answer questions and allow permissions, but not start or drive anything.                  |
| shared device             | Someone else's device paired with a host. It never acts as the person.                              |
| agent, lead, worker, team | One running agent CLI; one that started others; one another started; all of them.                   |
| board, card, Auto         | Where work waits; one piece of work; what carries a card to merged.                                 |
| sandbox                   | Where one card's work runs: a worktree or a virtual machine.                                        |
| trigger, event            | A rule that adds work when something happens; one time it happened.                                 |
| inbox, digest             | What waits on the person; what happened while they were away.                                       |
| vault                     | The account's encrypted store of keys, plugin sign-ins and site sign-ins.                           |
| agent sign-in             | An agent CLI's own login, kept on each host and never in the vault.                                 |
| site sign-in              | A website login the host's browser loads for agents allowed that site.                              |
| take over                 | Drive a host's browser tab from any device, to sign in or pass a check.                             |

Never "Mac", "computer", "machine" or "node" for a host, "client" for a device, or "session" for a
website login, which would read as a chat or terminal session.

## Processes

The desktop app is a Rust program drawn with [GPUI](https://www.gpui.rs). It is a client of
the core, a long-lived process that owns every shell, agent and project, and that phones and
other hosts reach too.

| Process           | Where it lives                         | What it does                                                                                                                                                                                                                                                                           |
| ----------------- | -------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Desktop app       | `app` (`sikemux`)                      | One window that draws the core's views, the browser tabs on its desks and the native views it places in its layout. See [The desktop app](#the-desktop-app).                                                                                                                           |
| Core              | `sikemux core` → `crates/sikemux-core` | Owns terminals, tasks, chat agents, Git, search, language servers, the harness and the agents' tool endpoint. It outlives the window; see [The core](#the-core).                                                                                                                       |
| `sikemux-host`    | `crates/sikemux-host` → `cli_client/`  | The `sikemux` launcher, which asks the running app to open files and projects. With `--tools-mcp` it is the MCP server agents launch over stdio; with `core` it is the core, with the built-in plugins. It never links the window's code, so a host with no screen installs only this. |
| Notch             | `helpers/notch` (Swift package)        | The island over a MacBook's notch, a client of the core that reads the view phones get. The app starts it; it keeps running after the app quits.                                                                                                                                       |
| `sikemux-voice`   | `helpers/voice` (Swift package)        | Speech to text, downloaded with the speech model. The `sikemux-voice` crate starts it and puts what it hears on a channel.                                                                                                                                                             |
| Simulator helper  | `helpers/sim` (Swift package)          | Streams an iOS Simulator's screen and takes its input, for the desk and the agents' simulator tools (`crates/sikemux-simulator`). Mac only.                                                                                                                                            |
| Shells and agents | spawned by the core                    | Children of the core, not of the app, so closing or quitting the app leaves them running.                                                                                                                                                                                              |

On macOS, `scripts/build-mac.sh` lays out the bundle: the GPUI app, the core as an app of its
own in `Contents/Helpers/Sikemux Core.app` (only an app can carry the provisioning profile that
lets it keep secrets in the keychain group it shares with the app) with a
`Contents/MacOS/sikemux-host` link for installed `sikemux` commands, and the notch helper.
`scripts/sign-mac.sh` signs it. On Linux, `scripts/build-linux.sh` builds the app with the core
beside it as a deb, an rpm and an AppImage. The voice and simulator helpers stay out of the
app: each release publishes them, and their crates download the one their build was made with.

### The core

The app is a client of the core. [ADR 0006](./0006-background-core.md) records why.

- **Starting.** The app starts the core on demand (`core::start` in the app), detached in its
  own session, logging to `core.log`. One core runs per user and channel; a lock file stops a
  second one. It exits by itself a while after it has no sessions and no client.
- **Talking.** A Unix socket, `~/.config/sikemux/core.sock` (`core.dev.sock` in dev builds,
  `SIKEMUX_CORE_SOCKET` to override), mode 0600. Frames are a length, a kind byte and a
  payload: requests, answers and events are JSON, terminal input and output are bytes. Every
  connection starts with a versioned handshake. What crosses is defined once, in
  `crates/sikemux-wire`.
- **What it owns.** Terminals and terminal agents (`server/session.rs`, on `sikemux-pty`),
  which it emulates and sends as screens of cells; chat agents (`server/chat/`), whose output
  it folds into rows (`crates/sikemux-fold`); tasks and their logs; Git, search, project files
  and language servers; the layout of every client's tabs (`server/layout.rs`) and the
  person's preferences; themes (`server/themes.rs`); the board, questions, the inbox and the
  agents' catalog; the harness, idempotency keys and the per-project event journal
  (`server/harness.rs`); and the agents' tool endpoint (`server/tools.rs`).
- **What clients do.** Clients draw what the core sends and send the person's actions back as
  the core's `Call`s (`server/calls.rs`). None reads an agent's messages or repeats the
  core's logic ([ADR 0010](./0010-phone-links-only-the-wire-and-client.md)).
- **Exits.** Closing the window and Quit only disconnect. Quit and Stop Everything stops every
  session and the core. Closing a pane, stopping a task or closing an agent still stops that
  one process.
- **Updates.** A newer app finds an older core by its build identity and asks it to update.
  The core writes its state to a private directory, keeps its sockets and terminal file
  descriptors open across `exec`, and becomes the new binary in the same process, so every
  shell stays its child.
- **Paired devices.** With remote access on, the core also listens on an iroh endpoint, and
  phones signed in to the same account join it from anywhere (`server/remote/`,
  `server/join.rs`). Each request is checked against what the device may do
  (`server/access.rs`). [ADR 0007](./0007-remote-access-over-iroh.md) records the design.
- **Linked hosts.** Hosts on one account link with each other and ask each other things over
  their own ALPN (`server/hosts/`): an agent here starts workers there, messages and questions
  cross the link, and a worker's commits come back to its lead's checkout.
  [ADR 0013](./0013-hosts-link-to-run-agents-for-each-other.md) records the design.

### The CLI broker

The core listens on a local TCP port guarded by a random token, and writes the port and token
to `~/.config/sikemux/cli.json` (`cli.dev.json` in dev builds). The `sikemux` launcher and the
tools MCP server find it through that file, or through `SIKEMUX_CLI_ENDPOINT`, which every
terminal Sikemux starts receives. A window registers with the core as the window
(`server/window.rs`), again whenever the person brings it forward; calls that need a window,
such as a file open or an agent's browser, go to the one that registered last.

## The desktop app

`app/` holds the GPUI app, the `sikemux-gpui` package. It shares the one Cargo workspace and
lockfile at the root, which builds it only when asked with `-p`. `src/main.rs` opens one
window; in a Wayland session it takes XWayland's display, since browser pages are X11 windows
inside it.

**One store, one direction.** `Workbench` (`workbench.rs`) is the one store: what the core last
said, and the little the window decides itself, such as focus, open menus, scroll and sizes.
`core.rs` talks to the core on a thread of its own, so the window's thread only draws. The
core's events go into the store, the store notifies views, and views draw; what the person does
goes out as `Call`s. No view keeps a copy of the core's data.

| Module                                                             | Owns                                                                                       |
| ------------------------------------------------------------------ | ------------------------------------------------------------------------------------------ |
| `workbench.rs`, `core.rs`                                          | The store and the link to the core                                                         |
| `rail.rs`, `tabs.rs`, `panes.rs`, `track.rs`                       | The rails, the tab strip, a tab's views split as the core's layout says, and their motion  |
| `chat/`                                                            | A chat: its rows in a transcript, the composer, attachments, mentions, edits, inline pages |
| `terminal.rs`                                                      | A terminal pane: the screen the core sends, drawn by `gpui-terminal-grid`                  |
| `editor/`, `syntax.rs`                                             | The editor tab on Kit's editor, highlighted by `textmate-highlight`                        |
| `git/`, `codehost/`, `search.rs`                                   | The Git tab, the project's code host in its place, and the Search pane                     |
| `desk.rs`, `sim/`                                                  | An agent's desk: its browser tabs, files, terminals and iOS Simulator                      |
| `board/`                                                           | The agent board and its card panels                                                        |
| `plugin_pane/`                                                     | Each plugin's pane (see [Plugins](#plugins))                                               |
| `settings/`, `experience.rs`, `welcome.rs`, `whats_new.rs`         | Settings, and the screens over the whole window                                            |
| `palette.rs`, `commands.rs`, `finder.rs`, `picker.rs`, `keymap.rs` | The pickers over the stage and the shortcuts                                               |
| `ui/`, `theme.rs`, `motion.rs`, `toast.rs`                         | The shared component set, the theme's tokens, motion and toasts                            |
| `notifications.rs`, `voice.rs`, `handoff.rs`                       | System notifications and the badge, dictation, and links opened from outside               |
| `drive.rs`                                                         | The in-app driver for the real-window checks (see [Tests](#tests))                         |

Its own crates are in `app/crates/`:

| Crate                | Purpose                                                                                                             |
| -------------------- | ------------------------------------------------------------------------------------------------------------------- |
| `gpui-transcript`    | A streaming chat view: rows by stable id, markdown patched block by block, a windowed list anchored to the bottom   |
| `gpui-terminal-grid` | Draws a terminal's grid of cells from cells emulated anywhere                                                       |
| `gpui-native-view`   | Shows a native view in an element's place, under the window's overlays                                              |
| `textmate-highlight` | Shiki's TextMate grammars, loaded lazily from one compressed bundle                                                 |
| `film-frame`         | Frames a filmed window for posts: the wallpaper, the window's shadow, a camera, a drawn cursor and the closing card |

### GPUI, Kit and themes

GPUI is our fork, `sikemux/gpui` (its `sikemux` branch), patched in under the `gpui-pre` names
[GPUI Kit](https://crates.io/crates/gpui-kit) pins; the rev is in the workspace's
`[patch.crates-io]` and its lockfile. Screens are built from `ui/`, which wraps Kit's
components; a screen that needs something new adds it to `ui/`. The core resolves the person's
theme into one set of tokens, and `theme.rs` turns them into GPUI colours and Kit's theme.
Read [DESIGN.md](../../DESIGN.md) before changing anything visual.

### The browser engine

`crates/sikemux-browser` is the browser with no UI toolkit: tabs, their pages, history,
downloads and the agents' browser tools (`tools.rs`, declared in `browser/tools.json`). The
desk holds a `BrowserManager` and hands it the window to put pages in; the app answers the
core's browser calls by running them on it.

- **macOS** (`macos/`): each tab is a WKWebView added to the window's view, under the
  overlay plane GPUI draws menus on.
- **Linux** (`linux/`): each tab is WebKitGTK through
  wry, in an X11 window of its own inside the app's, sitting in a clip window that cuts it
  where the stage ends. GTK runs on a thread of its own. Our GPUI's X11 window cuts native
  child windows where its menus draw, and lets the pointer through where they take it. Before
  GTK starts, `choose_page_rendering` turns off WebKit's DMA-BUF renderer where the X server
  has no DRI3 or there is no render node, since pages would stay blank there.
- **Pages agents show** (`page_view/`) draw inline in a chat reply, in a view of their own
  that keeps no storage and never navigates.

### Native views

`gpui-native-view` places something native where an element is laid out: a page, a PDF, a
document. `native_view(id, surface)` lays out like any element and hands its `Surface` its
frame, cut to what its ancestors let show, each time that changes. On macOS `OwnedView` adds
any `NSView`, and `document.rs` draws PDFs in PDFKit and office files in Quick Look; on Linux
`window_x11` gives the window to parent X11 windows to. A window that holds native views calls
`enable_scene_overlay` as it opens, so menus and popovers show above them.

## Crates (`crates/`)

| Crate                | Purpose                                                                                                                                                                                    |
| -------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `sikemux-core`       | The core: its socket server, terminals, tasks, chat agents, the harness and tool endpoint, updating in place                                                                               |
| `sikemux-host`       | The `sikemux` executable: the launcher, the tools MCP server and the core with the built-in plugins                                                                                        |
| `sikemux-base`       | What the core shares with the browser, simulator and voice crates: HTTP clients, pinned downloads, the async runtime, page building and the CLI endpoint's protocol                        |
| `sikemux-wire`       | What the core and its clients send each other. The phone links it, so it holds data and nothing that acts on it                                                                            |
| `sikemux-client`     | Talking to a core: on this host, from a paired device, and joining a host. The phone links it                                                                                              |
| `sikemux-fold`       | A chat folded from what its agent says into the rows clients draw                                                                                                                          |
| `sikemux-pty`        | The terminal engine: each PTY's screen, shell integration, the login-shell environment, task checks and `agent_detection/`, which reads an agent's screen to tell working, blocked or idle |
| `sikemux-browser`    | The browser engine (see [The browser engine](#the-browser-engine))                                                                                                                         |
| `sikemux-simulator`  | The iOS Simulator for agents and the desk: the device helper and the agents' tools                                                                                                         |
| `sikemux-linked`     | Reaching the account's other hosts as a device of the person's own                                                                                                                         |
| `sikemux-markdown`   | Parses markdown into the block tree the transcript draws; `sikemux-markdown-wasm` builds it for the web                                                                                    |
| `sikemux-plugin-api` | The contract between the core and a plugin: a method name and JSON in, JSON or a stream out                                                                                                |
| `sikemux-keychain`   | Secrets in the system keychain, for the plugins that sign in                                                                                                                               |
| `sikemux-loopback`   | Waits for the browser to come back from a sign-in, on one port where each plugin has its own path                                                                                          |
| `sikemux-process`    | Runs a subprocess with a timeout and an output limit, and the paths and environment Sikemux uses                                                                                           |
| `sikemux-voice`      | Dictation for every app, over the `sikemux-voice` helper                                                                                                                                   |
| `sikemux-mobile`     | The core's client as the phone app calls it (see [Mobile app](#mobile-app-mobile))                                                                                                         |
| `workspace-hack`     | Keeps every build in the workspace on one set of dependency features, managed by hakari                                                                                                    |

## Plugins

Plugins are compiled in; there is no marketplace and no runtime loading
([ADR 0002](./0002-plugins-are-compiled-in.md)). The twelve built-ins are AWS, Bitbucket, Bruno,
Database, GitHub, GitLab, Jira, Linear, Rundeck, SigNoz, Slack and Vercel.

- **In the core:** a crate in `plugins/<name>/` implements the `Plugin` trait from
  `sikemux-plugin-api`. `sikemux-host` gives each one a Cargo feature of the same name, all on
  by default, and `crates/sikemux-host/src/plugins/builtin.rs` lists them. Plugin crates opt
  in to the workspace Clippy lints that forbid `unwrap`, `expect`, `panic` and unchecked
  indexing, because a panicking plugin takes the core down.
- **In the app:** the core reads the views a plugin's manifest names and says when one
  changed (`server/plugin_views.rs`); `plugin_pane/` keeps what it was last sent and hands it
  to the plugin's own panel, `plugin_pane/<name>.rs`, to draw. What a panel changes goes back
  through the plugin's calls.

A plugin can also offer agent tools, which the MCP server lists next to the built-in ones.
Settings can switch a plugin off without removing it from the build.

## Agents

An agent runs on one of two transports.

- **Terminal.** The agent's own CLI runs in a PTY like any shell (`sikemux-pty`'s
  `launch.rs`). Sikemux tells its state from the screen (`agent_detection/`) and finds its saved
  sessions on disk.
- **Chat.** Chat agents run in the core, so a turn keeps going while no window is open.
  Claude and Codex are driven in their own protocols (`server/chat/native/claude` speaks Claude
  Code's stream-json control protocol, `server/chat/native/codex` speaks `codex app-server`);
  OpenCode, OMP, Grok and Hermes speak [ACP](https://agentclientprotocol.com) themselves. Every
  driver feeds the same session updates, which `sikemux-fold` folds into rows; every client
  draws those rows.

**Accounts.** A Claude or Codex account is a profile whose directory holds one sign-in. The
core makes extra ones, signs in and out through the CLI's own login, starts a chat again on the
same session when its account's sign-in changes and, when the person turned it on, moves a chat
at a usage limit to the next signed-in account.

**Harness tools.** Agents on both transports get Sikemux's tools through
`sikemux-host --tools-mcp`. Tools are declared once in `browser/tools.json`, with the details in
`browser/SIKEMUX_GUIDE.md`, which agents fetch with the `guide` tool. An agent keeps its MCP
process across an update, so the process takes both from the core and uses its own copy only
when the core cannot be reached. A tool call travels:

```
agent → sikemux-host --tools-mcp → the core's tool endpoint → server/harness.rs
  task.*, events.wait, agent.*, board.*, git.commit, plugins.*, … → answered by the core
  browser.*, sim.*, app.console, page.show, ui.open              → the window that registered
    browser.*                                                    → sikemux-browser's tools.rs
    sim.*                                                        → sikemux-simulator's tools
```

The core keeps runs, idempotency keys and an event journal per project, so task tools work with
no window open and event cursors survive restarts. `pnpm agent-tools:generate` writes
`crates/sikemux-base/src/cli/methods.rs` and checks that every declared method has a handler.

## Tests

- **Rust tests** run with `cargo test` in the one workspace, whose default members are the core's
  crates and the plugins; the GPUI app and its crates are named with `-p`. CI runs both on macOS
  and the core's crates' on Linux.
- **Headless shots.** `sikemux-gpui`'s `shot` example draws the window offscreen into a PNG,
  optionally after a list of steps (clicks, keys, waits), to check a screen without a person at
  it. `fake_chat` seeds a throwaway core with chats from the fake agents in
  `crates/sikemux-core/src/bin/` (`sikemux-fake-acp-agent` and others), which answer prompts
  by script and never reach a real agent CLI.
- **Real windows.** Built with `--features drive`, the app listens for steps from the checks in
  `app/ci/` (`drive.rs`) and reports what it shows. `desk-checks.py` drives the desk on
  macOS; `linux-window.py` opens the app under Xvfb with a window manager, drives the agents'
  browser tools on a local page and checks the pages drew; `ground-check.py`, `sky-check.py` and
  `trust-check.py` check the window's ground, What's New's sky and how the core trusts the app.
  Runs away from CI start only under the agent guard, so no real agent CLI can start.
- **Benches.** `stage_bench`, `chat_bench`, `terminals` and `stress` time the hot paths.

## Mobile app (`mobile/`)

The phone app pairs with a host's core and drives it over iroh. It is an Expo app (`mobile/app`)
that calls the core's own client through `@sikemux/native` (`mobile/native`), which
`uniffi-bindgen-react-native` generates from `crates/sikemux-mobile`. That crate links only
`sikemux-wire` and `sikemux-client`, so their code decides when a phone needs a store build
([ADR 0010](./0010-phone-links-only-the-wire-and-client.md)). TypeScript the phone and the web
app share lives in `shared/`. [ADR 0008](./0008-mobile-app-expo-and-rust-client.md) records the
choices.

## Generated files and checks

`pnpm check` runs, among others, these checks that fail when generated output is stale. Run the
matching `:generate` script and commit what it writes:

| Check                            | What it compares                                                                                                                        |
| -------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------- |
| `wire:check`                     | The types clients read the core's JSON with (TypeScript for the phone and web app, Swift for the notch) against the Rust that writes it |
| `agent-tools:check`              | `crates/sikemux-base/src/cli/methods.rs` against `browser/tools.json`                                                                   |
| `agent-catalog:check`            | The core's Rust copy of the agents' catalog against `sikemux-core/src/assets/agent-catalog.json`                                        |
| `docs:reference:check`           | The docs site's reference pages against the shortcuts, CLI help and tools                                                               |
| `grammars:check`, `syntax:check` | The grammar list and the GPUI app's highlighting bundle                                                                                 |
| `faces:check`                    | The agents' faces, from the art in `shared/buddies`                                                                                     |
| `file-icons:check`               | The file icons the app and the web app draw                                                                                             |
| `notch-sources:check`            | The settings, colours and faces the notch helper shares with the app                                                                    |

`release:check` runs offline checks of the release configuration, which reads the app's name,
identifiers and version from the GPUI crate's `Cargo.toml`. `pnpm rust:fmt:check`,
`pnpm rust:clippy` and `pnpm rust:test` cover the Rust; CI is the gate.
