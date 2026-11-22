# Memory at idle with 20 agent sessions

Measures how much memory and CPU an agent desktop app holds after it opens 20 real Claude Code
sessions, gets one reply in each, and sits idle for 10 minutes.

No tokens are spent. Every session is the real `claude` CLI, pointed at `fake-anthropic.mjs`, a
stand-in for Anthropic's Messages API on `127.0.0.1` that answers each turn with `Ready.`

## What you need

- macOS, with its own `footprint` (no `sudo`: it reads only your own processes).
- Node.js 18 or later. No npm packages.
- Claude Code: `claude` on your login shell's `PATH` (`zsh -lc 'command -v claude'` prints it).
- The apps, in `/Applications` or anywhere with `--bundle`:
  - `Sikemux.app`
  - `T3 Code (Nightly).app`, or `T3 Code.app` when the nightly is not there
  - `Orca.app`
  - `Conductor.app`
- A macOS user named `bench`. Orca and Conductor write into the home folder's agent settings
  (`~/.conductor/settings.managed.toml`, Claude Code's trust list), so `run.mjs` runs them only
  as `bench`. Sikemux keeps its workspaces in the account's Application Support folder, which no
  setting moves, so a Sikemux run leaves its project and 20 chats in that account's Sikemux;
  run it as `bench` too to keep your own clean. Log every other user out first, and put `node`
  and `claude` on `bench`'s `PATH`.

## Running

Quit the app first. A run starts it fresh, opens the sessions, waits until every one has
answered, samples memory and CPU through the idle window, writes `results/<app>.json`, and
quits the app.

```bash
node scripts/bench/run.mjs all          # every installed app, one after another
node scripts/bench/run.mjs t3code       # one app
node scripts/bench/run.mjs orca --bundle ~/Applications/Orca.app
```

`all` skips, with a line saying why, an app that is not installed and, unless you are `bench`,
Orca and Conductor. It carries on past an app that fails and exits non-zero at the end.

| Option | Default | |
| --- | --- | --- |
| `--sessions N` | `20` | Claude Code sessions to open |
| `--idle S` | `600` | seconds measured once every session has answered |
| `--interval S` | `30` | seconds between samples while idle |
| `--out DIR` | `scripts/bench/results` | folder for `<app>.json` |
| `--bundle PATH` | | the app to run, for one app installed elsewhere (not with `all`) |
| `--keep` | | keep the run's temporary folder (the repository, Claude config and app data) |
| `-h`, `--help` | | list the options |

Then build the page:

```bash
node scripts/bench/page.mjs [results dir] [out.html]
```

It reads whichever of `sikemux.json`, `t3code.json`, `orca.json` and `conductor.json` are there
and writes one HTML page: bars, numbers, the method, what differs between the apps, and every
process.

`measure.mjs` also measures an app that is already open:

```bash
node scripts/bench/measure.mjs --app /Applications/Orca.app --idle 600 --interval 30 --out orca.json
```

## Each app

- **Sikemux**: sessions start with `sikemux-host chat start`. The first time that runs from
  outside a Sikemux terminal in an account, Sikemux asks for Allow in its window; click it once.
- **T3 Code**: runs with its own `T3CODE_HOME` and `CLAUDE_CONFIG_DIR` in the run's temporary folder.
- **Orca**: runs with its own `ORCA_USER_DATA_PATH` and `CLAUDE_CONFIG_DIR`. Each worktree is
  trusted in that Claude config before its terminal `claude` starts.
- **Conductor**: nothing starts its sessions from outside. The run points
  `~/.conductor/settings.managed.toml` at the fake API, opens Conductor, and prints a repository
  path and a first message (`bench session 1: reply with one word.`). Add that repository (⌘⇧G in
  the Open dialog takes the `/var/folders/...` path) and send that message; the fake API then has
  that session open sessions 2 to 20 with Conductor's own CreateWorkspace tool. The settings file
  is put back as it was when Conductor quits.

## What is measured

- **Memory** is physical footprint, what Activity Monitor's Memory column shows, from
  `footprint`, with memory shared between the measured processes counted once. Summing RSS
  would count shared pages twice and miss compressed memory.
- **App** is every process of yours whose program is inside the app bundle (main, helpers,
  renderers, servers, daemons) and whatever those start that is not an agent: shells, git,
  language servers.
- **Agent CLIs** are the `claude` processes the app started and everything under them, also when
  the app's own binary runs Claude Code's `cli.js`, as Electron apps do.
- **Together** measures both sets in one `footprint` call, so pages they share count once. The
  peak over the idle window is kept too.
- **CPU** is the CPU time each set used over the idle window, as a percentage of one core.

## What differs between the apps

Each app runs in its default workflow. The page states these:

1. Sikemux and T3 Code open all sessions in one project folder; Orca and Conductor give each
   session its own git worktree.
2. Orca runs each session as Claude Code's terminal interface in a shell; the others drive
   Claude Code without a terminal.
3. Apps stop idle agents on their own schedule. Sikemux puts an idle chat to sleep after 2
   minutes unless its answer is still unread, and keeps at most 3 idle chats awake that no
   window shows, sleeping the oldest beyond that even when unread. T3 Code stops one after 30
   minutes. Orca and Conductor keep theirs. The page shows how many `claude` were still running
   at the end beside each number, and the peak before any went to sleep.
4. Conductor's first session is started by hand, and it opens the rest itself. It runs the same
   `claude` as the others, through its `claude_executable_path` setting.
5. Each app gives its agents its own prompts and tools, which changes what each `claude` holds.

## Files

- `run.mjs` makes the repository, starts the fake API and the app, opens the sessions, measures, and quits.
- `fake-anthropic.mjs` answers `/v1/messages`, `/v1/messages/count_tokens` and `/v1/models`, and
  counts the sessions that reached it at `/bench/stats`, each by the first user message of its request.
- `measure.mjs` sorts the process tree, samples `ps` and `footprint`, and writes the JSON.
- `page.mjs` turns the JSON files into the HTML page.
