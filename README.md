<div align="center">

# Sikemux

**A desktop workspace for terminals, code, Git, coding agents, cloud tools, deployments, and API collections. Built with Tauri, Rust, and React.**

![Sikemux editor](public/screenshots/project-editor-view.png)

[![macOS](https://img.shields.io/badge/macOS-11%2B%20Apple%20Silicon-000?logo=apple&logoColor=white)](#installation)
[![Tauri 2](https://img.shields.io/badge/Tauri-2-24C8DB?logo=tauri&logoColor=white)](https://tauri.app)
[![React 19](https://img.shields.io/badge/React-19-61DAFB?logo=react&logoColor=black)](https://react.dev)
[![Rust](https://img.shields.io/badge/Rust-backend-000?logo=rust&logoColor=white)](https://www.rust-lang.org)
[![Latest release](https://img.shields.io/github/v/release/nodelike/sikemux?display_name=tag)](https://github.com/nodelike/sikemux/releases/latest)

</div>

---

## What it does

Sikemux puts the tools tied to a terminal project in one window. Open a project once. Its files, shells, Git state, agents, cloud resources, deployments, and API collections stay attached to that project.

### Projects

Each project has five views named `Files`, `Term`, `Git`, `Agents`, and `Search`. They all use the same working directory.

The editor uses CodeMirror 6 and supports JavaScript, TypeScript, JSX, Python, Rust, Go, HTML, CSS, JSON, YAML, Markdown, and legacy modes. LSP support covers hover, go to definition, and peek. The editor also has project-wide Problems and Outline panels, a Git gutter, find and replace, indentation guides, and virtualized rendering for large files.

Sikemux includes a side-by-side diff editor and a three-way merge view for resolving conflicts. The file tree watches the filesystem for changes. You can create, rename, delete, and move files, or drop files in from Finder.

<table>
<tr>
<td width="50%"><img src="public/screenshots/project-term-view.png" alt="Terminal view"/></td>
<td width="50%"><img src="public/screenshots/project-git-view.png" alt="Git view"/></td>
</tr>
<tr>
<td align="center"><b>Terminal.</b> xterm.js backed by native Rust PTYs, with split panes, tabs, path drag and drop, and optional WebGL rendering.</td>
<td align="center"><b>Git.</b> Branches, staging, commits, diffs, merges, pull and push, worktrees, and commit messages generated through a local CLI.</td>
</tr>
</table>

### Project actions, tasks, and previews

A project can check in a bounded `sikemux.json` file. Sikemux puts its actions and tasks in the command deck, opened with `⌘⇧P` on macOS or `Ctrl+Shift+P` on Windows. Actions can also define project-specific shortcuts.

The only accepted action context is `"project"`. An empty context list makes the action available throughout its project. Sikemux shows the command before it runs and remembers approval only for the current app process. If the file changes, Sikemux asks again.

```json
{
  "version": 1,
  "actions": [
    {
      "id": "quality",
      "label": "Run quality checks",
      "description": "Format, lint and test",
      "command": "pnpm check",
      "placement": "terminal",
      "contexts": ["project"],
      "keybinding": "Meta+Shift+KeyT"
    }
  ],
  "tasks": [
    {
      "id": "dev",
      "label": "Development server",
      "command": "pnpm dev",
      "cwd": ".",
      "env": { "NODE_ENV": "development" }
    }
  ],
  "preview": {
    "url": "http://localhost:5173",
    "command": "pnpm dev"
  }
}
```

Project shortcuts use physical key codes. Use `Meta` on macOS and usually `Ctrl` on Windows. The `env` object lives in the checked-in file, so do not put secrets there.

The Rust runtime owns each task's native PTY. The renderer only displays it. You can detach and reattach the terminal view without stopping the process. Restarts and stops target the correct task generation. The Stop action remains available if the configuration disappears or becomes invalid while a task is running. Sikemux does not write task environment values to saved terminal state or diagnostics.

### Coding agents

Sikemux detects Claude, Codex, Hermes, Pi, and OpenCode on your `PATH`. It reads their saved sessions and can run several agents in one project, each in its own pane.

- Open an agent picker with `⌥N` from the Agents view. Sikemux launches the selected CLI directly in a PTY.
- Choose Normal or YOLO mode. Press `⌥Y` to toggle the mode for a resumable agent.
- Press `⌘T` to split the active agent pane and open the embedded browser. Browser tabs belong to the session and keep their sign-ins. You can take control at any time. Agent commands run through your configured interactive shell and use its environment and login. Browser tools use the agent’s own MCP configuration.

![Agents view](public/screenshots/project-agents-view.png)

### Agent harness tools

The bundled browser MCP also exposes six Sikemux tools. They operate on the agent's open project and do not require starting Chromium. The native CLI exposes the same operations with JSON input and output:

```bash
sikemux tool workspace.inspect
sikemux tool task.start '{"taskId":"dev","idempotencyKey":"dev-first-run"}'
sikemux tool task.read '{"executionId":"RETURNED_ID","cursor":0}'
sikemux tool events.wait '{"cursor":"RETURNED_EVENT_CURSOR","timeoutMs":30000}'
sikemux tool ui.open '{"kind":"file","path":"src/App.tsx","line":42,"focus":true}'
sikemux tool task.stop '{"executionId":"RETURNED_ID"}'
```

| CLI method          | MCP tool                    | Behavior                                                                         |
| ------------------- | --------------------------- | -------------------------------------------------------------------------------- |
| `workspace.inspect` | `sikemux_workspace_inspect` | Project panes, configured tasks, harness runs, and the current event cursor      |
| `task.start`        | `sikemux_task_start`        | Start a configured task in a managed terminal; requires an idempotency key       |
| `task.read`         | `sikemux_task_read`         | Status, exit code, and output after a byte cursor                                |
| `task.stop`         | `sikemux_task_stop`         | Stop one exact execution and its process tree                                    |
| `ui.open`           | `sikemux_ui_open`           | Open a file, diff, task terminal, or the configured preview                      |
| `events.wait`       | `sikemux_events_wait`       | Wait up to 30 seconds for project task output, task lifecycle, or UI-open events |

Task launches use `sikemux.json` and its existing project-trust dialog. A changed configuration is checked again before launch. Reusing an idempotency key returns the original execution, including after completion. Starting an already active harness task returns that execution. A task already running through the command deck must be stopped there before launching the same task through the harness.

Task terminals open in the background and remain available in the project. Use `ui.open` with `kind: "terminal"`, an `executionId`, and `focus: true` to reveal one. Files open as background tabs by default. Set `focus: true` to reveal the file at the requested one-based line. Files must resolve inside the project. Preview opens require an agent session and use that agent's browser. A returned `previewUrl` is configured information; inspect task output or use browser tools to verify readiness.

Pass the returned output cursor into the next `task.read`. `hasMore` means another page is available. Output pages default to 8 KiB, with a supported `limit` of 4–8192 bytes, and may contain terminal escape sequences. Each task retains up to 1 MiB of raw output; `truncated` reports when a cursor predates retained bytes. Completed and stopped PTYs use the native ten-minute retention period and may be evicted earlier under capacity pressure.

Event cursors are separate from output cursors. Inspect the workspace to obtain an event cursor, then pass it to `events.wait`. An optional `executionId` filters the wait. A timeout returns an empty event list and a new cursor. Event history holds 256 entries; `truncated` means the caller should inspect current state again. Waits do not schedule future agent turns.

Harness run history and idempotency keys last for the current frontend session, with limits of 128 runs and 256 keys. App restart does not resume commands, and a frontend reload loses harness run handles. Closing a project stops its harness tasks. The local authenticated endpoint follows `SIKEMUX_CLI_ENDPOINT`; terminals launched by Sikemux also provide project and agent context. From an external shell, run the CLI inside the open project's Git root or set `SIKEMUX_PROJECT` explicitly.

Run the native smoke with `pnpm test:e2e:desktop`. To also exercise managed tasks, run `node scripts/smoke-desktop-e2e.mjs --tasks` after that build and approve the temporary fixture project in its isolated Sikemux window. This checks launch deduplication, live event waits, output cursors, retained logs, exact execution stops, and failure exit codes. The fixture is removed afterward.

### AWS

The AWS panel shows:

- Cost and billing
- ECS services and tasks, including selectable and copyable CloudWatch log output while it streams
- EC2, Lambda, S3, and SQS

Use the refresh action to load changes made in the AWS console.

<table>
<tr>
<td width="50%"><img src="public/screenshots/cloud-aws-billing-view.png" alt="AWS billing"/></td>
<td width="50%"><img src="public/screenshots/cloud-aws-ecs-tasks-logs-view.png" alt="AWS ECS tasks and logs"/></td>
</tr>
</table>

### Rundeck

The Rundeck panel lets you browse projects, start jobs from a palette, and follow each deployment step and its output as it runs.

Sikemux uses HTTPS by default. You can allow HTTP when signing in to a Rundeck installation on a private subnet. Sikemux accepts it only if every resolved address is private, loopback, or link-local. It pins those verified addresses in the credential and token client to block DNS rebinding. Sikemux stores the acknowledgement beside the token configuration and saves that file with mode `600`.

![Rundeck](public/screenshots/cicd-rundeck-projects-view.png)

### Bruno

Open a [Bruno](https://www.usebruno.com/) collection as its own session. Use `⌘P` for the request palette and `⌥E` for environments. Save with `⌘S` and send with `⌘↵`. Pre-request and post-request hooks run in a scripting sandbox.

![Bruno](public/screenshots/api-bruno-pane-view.png)

### SSH and command sessions

Press `⌥⇧S` to connect to an SSH host. Press `⌥S` to open a scratch command shell. Both open as multiplexed sessions.

### Command-line editor integration

Packaged builds include a native `sikemux` CLI. It sends files and project directories to the running app and accepts editor-style line and column positions. The `--wait` flag blocks until you close the opened tab, which means Git can use Sikemux as its editor.

Install the launchers from Settings → CLI. Sikemux does not replace unrelated files or edit shell startup files.

```bash
sikemux .
sikemux src/App.tsx:42:5
sikemux open --wait README.md
EDITOR=sikemux-editor git commit
```

Terminals opened by Sikemux set `TERM_PROGRAM=Sikemux`, `SIKEMUX=1`, the app version, and typed context for the current session, project, pane, or agent. If neither `EDITOR` nor `VISUAL` is set, Sikemux points both to its bundled wait-enabled editor CLI. It leaves existing values alone.

### Themes and window controls

Sikemux includes nine themes: Aura, Ayu Dark, Tokyo Night, Catppuccin Mocha, Dracula, Gruvbox Dark, Nord, One Dark, and Solarized Dark.

The custom theme editor covers the interface, editor, syntax colors, and all 16 terminal colors. Window controls include a frameless overlay title bar, adjustable transparency and blur through a private macOS API, and Zen mode.

### Pane management, updates, and diagnostics

You can tile and split panes, move focus with Vim-style shortcuts, find sessions with fuzzy matching, and move backward or forward through editor history. Sikemux saves each layout change as a transaction and restores the layout after a restart. The built-in updater reports new releases.

Optional shell integration reports the current directory, command phase, and last exit status without editing shell startup files. Diagnostics combine redacted browser and native traces, latency percentiles, subsystem counts, and an operating-system thread watchdog. The watchdog can save evidence when the WebView stalls.

## Keyboard shortcuts

These are the defaults. You can reassign or clear every command in Settings → Keybindings. Changes save automatically.

| Key          | Action                           |     | Key              | Action                        |
| ------------ | -------------------------------- | --- | ---------------- | ----------------------------- |
| `⌥S`         | Open or create a session         |     | `⌥\` / `⌥-`      | Split pane by row or column   |
| `⌥P`         | Open project                     |     | `⌥H/J/K/L`       | Move focus between panes      |
| `⌥⇧S`        | Connect to an SSH host           |     | `⌥⇧H/J/K/L`      | Resize active pane            |
| `⌥A`         | Open AWS                         |     | `⌥Z`             | Zoom or unzoom pane           |
| `⌥B`         | Open a Bruno workspace           |     | `⌥W`             | Close focused pane            |
| `⌥1` to `⌥5` | Files, Term, Git, Agents, Search |     | `⌥Tab` / `⌥⇧Tab` | Cycle session or group        |
| `⌥[` `⌥]`    | Previous or next window          |     | `⌥N`             | Open terminal or agent picker |
| `⌘P`         | Open file or request palette     |     | `⌥Y`             | Toggle agent YOLO mode        |
| `⌘⇧F`        | Search project                   |     | `⌥U`             | Open last-used session        |
| `⌘⇧P`        | Open command deck                |     | `⌥T`             | Focus command terminal        |
| `⌘T`         | Open embedded browser tab        |     | `⌘L`             | Focus browser address         |
| `⌘,`         | Open settings                    |     | `Esc`            | Dismiss active modal          |

On Windows, use `Ctrl` for shortcuts marked `⌘` and `Alt` for shortcuts marked `⌥`.

## Installation

### Download

Download the latest `.dmg` from [Releases](https://github.com/nodelike/sikemux/releases/latest). Published releases support Apple Silicon and require macOS 11 or later. The updater keeps an installed copy current. The published updater feed does not cover Intel Macs.

### Build from source

You need [Rust](https://www.rust-lang.org/tools/install), Node.js 22 or later, [pnpm](https://pnpm.io/), and [uv](https://docs.astral.sh/uv/getting-started/installation/) for the embedded Browser Use sidecar.

```bash
git clone git@github.com:nodelike/sikemux.git
cd sikemux
pnpm install

# Run Vite and Tauri with hot reload, separate from the installed app
make dev
# Or run pnpm dev:desktop

# Build an Apple Silicon app and DMG on an Apple Silicon Mac
make build
# Or run pnpm build:mac

# Build a Windows NSIS installer from Windows
pnpm build:windows
```

Sikemux generates commit messages through a locally installed Hermes, Codex, or Claude CLI. It does not call a model provider directly and does not need its own API key. Choose the CLI and model in the commit panel. The selected CLI streams generated text into the commit box.

Claude's partial-message stream and Codex's local app server send token-level text updates. Sikemux forwards Hermes output when its quiet CLI mode flushes. For large changes, it sends zero-context diffs within the selected model's budget while covering every file and hunk. It summarizes generated files and lockfiles instead of sending their full contents.

Windows development requires Microsoft C++ Build Tools and WebView2. Sikemux uses native ConPTY and defaults to PowerShell on Windows.

The terminal defaults to xterm.js's DOM renderer. Set `VITE_TERMINAL_WEBGL=1` to test WebGL.

```bash
VITE_TERMINAL_WEBGL=1 pnpm dev:desktop
```

Development builds use the separate `com.nodelike.sikemux.dev` identity. They can run beside the installed app without triggering its single-instance guard or sharing its application data. If WebGL fails to start or loses its context, Sikemux switches back to DOM rendering. Run `window.sikemuxDiagnostics?.snapshot()` in the WebView console to see renderer counts.

Run the full local quality gate with:

```bash
make check
```

It runs Prettier, ESLint, TypeScript checks, deterministic frontend tests, Rust formatting, Clippy, Rust tests, and release-tooling checks that need no credentials. `make test-coverage` reports coverage for all frontend TypeScript and TSX files, including files that no test imports. `make run` launches an existing release build.

### Community releases without an Apple Developer membership

The updater and Apple Gatekeeper trust different signatures. By default, `scripts/release.sh` makes a community release. It signs the updater archive with the Tauri updater key and applies an ad hoc code signature to the app and DMG.

Existing community installations can receive in-app updates. Fresh downloads are not notarized by Apple, so macOS may ask you to remove quarantine again. Keep the updater private key secure. Clients reject archives that do not match the public key bundled with the app.

Stable builds create a versioned GitHub release and update `latest.json`. Preview builds require a prerelease semantic version and update the moving `preview` release used by the opt-in Preview channel.

```bash
./scripts/release.sh 0.2.0 "Release notes" --publish
./scripts/release.sh 0.3.0-beta.1 "Preview notes" --preview --publish
```

Leave out `--publish` to build, sign, and verify the release without changing GitHub.

If you have an Apple Developer membership, set `RELEASE_NOTARIZED=1` with the Developer ID and notarization environment variables. The release script then requires a successful Gatekeeper assessment and stapled notarization tickets before it publishes anything.

## Contributing

Issues and pull requests are welcome. [CONTRIBUTING.md](CONTRIBUTING.md) covers setup, the project layout, and checks to run before opening a pull request.

## License

[MIT](LICENSE) © nodelike

---

<div align="center">
<sub><code>sike</code> + <code>mux</code>, built by <a href="https://github.com/nodelike">@nodelike</a></sub>
</div>
