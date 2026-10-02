---
name: sikemux-harness
description: How to drive a Sikemux project and its browser — task launches, output and event cursors, opening UI, and the tab model.
---

# Working inside Sikemux

You are running in a pane of a Sikemux workspace. `workspace_inspect`, the
`task_*` tools, `ui_open` and `events_wait` act on the project the person has
open. The tools named `browser_*` act on the browser tabs on your desk, the
pane beside yours that the person can see. Your desk holds everything you open
for the person: browser pages, files and the terminals of tasks you start, as
tabs in one strip.

When the person asks you to open, show or preview a page, open it on your desk
with `browser_navigate`, not in their own browser. Desk tabs run WebKit, the
same engine as Sikemux itself and Safari, so check a web page there rather
than in a headless Chromium you install.

This page is what to know before your first call. Call `guide` again with a
`topic` from the list at the end when you need the details of one area.

## Essentials

- `workspace_inspect` comes first. It returns the open project, the tasks in
  `sikemux.json`, the runs you already started and an event cursor. Task ids
  come from there; do not guess one. Its `ports` lists the TCP ports the
  project's terminals, tasks and agents listen on, preview first.
- `task_start` takes an `idempotencyKey` you choose and either a `taskId` or a
  `command`. Reusing a key returns the original run, so pick one key per
  attempt and reuse it when a call fails or you are unsure it landed. Use
  tasks for dev servers and watchers rather than background shell jobs.
- `task_read` pages output by byte cursor: start at `0` and pass back the
  `cursor` it returns. `plain: true` strips terminal escapes, `tail: N` gives
  the last lines and `search` the matching ones.
- `events_wait` takes an event cursor from `workspace_inspect` or the last
  wait. **Event cursors are not output cursors**; never mix them. A timeout
  is normal: wait again with the fresh cursor. Cursors stay valid after
  Sikemux reloads or restarts.
- Browser tools that act report what changed since your last read of the
  page (`report: "changes"`); `"outcome"` is leaner and `"full"` returns the
  whole state. The first read of a new page is always full.
- **Element numbers expire** when their element leaves the page, and
  numbering restarts on every new page. Never carry a number across a
  navigation; read or `browser_find` again.
- Wait with `browser_wait` conditions, or `waitFor` on `browser_navigate`,
  rather than sleeps or screenshots taken to see whether something finished.
- Every state says whether the tab is `visible` to the person; do not tell
  them a page is on their screen when it is not.
- Plugin tools (`github_*`, `bitbucket_*`, `signoz_*`) are listed only when
  they can work here: signed in, and for GitHub or Bitbucket a remote of this
  project on that host. If one you need is missing, ask the person to sign in
  from its pane in Sikemux and restart you.

## Topics

Pass one of these as `topic`:

- `config` — writing or fixing `sikemux.json`, and what `configStatus` means
- `tasks` — launching, trust prompts, `readyWhen`, restarting, stopping, and what survives a reload
- `output` — `task_read` paging, `plain`, `tail`, `search`, and `events_wait`
- `ui` — `ui_open` for files, diffs, terminals and the preview
- `browser-reading` — page state, report modes, element numbers and lines, `browser_find`, frames
- `browser-input` — clicking, typing, `browser_act`, keys, drag, upload, scrolling, dialogs
- `browser-pages` — navigating, reloading, waiting, local files, viewport sizes, tabs
- `browser-evidence` — `browser_screenshot`, `browser_annotate`, `browser_record`
- `browser-debugging` — `browser_network`, loads, `browser_console`, `app_console`, `browser_evaluate`
- `shell` — the `sikemux tool` CLI for scripts and tasks

## config: Writing sikemux.json

`configStatus` is `absent` when the project has no `sikemux.json`, and
`invalid` when it has one that failed to load; `configErrors` then lists each
problem by path. Neither state offers any tasks.

The file sits at the project root. Tasks are what `task_start` launches:

```json
{
  "version": 1,
  "tasks": [
    { "id": "web", "label": "Web", "command": "pnpm dev", "cwd": "apps/web" },
    { "id": "api", "label": "API", "command": "uv run main.py", "env": { "PORT": "8000" } }
  ],
  "preview": { "url": "http://localhost:5173" }
}
```

`version` is required and must be `1`. A task needs `id`, `label` and
`command`; `cwd` is relative to the project and defaults to `.`, and `env`
holds string values. Ids use letters, numbers, `.`, `_` and `-`. Unknown fields
are rejected. If the project already describes its processes elsewhere, such
as `.claude/launch.json` or a Procfile, carry those commands over rather than
inventing new ones. Inspect again after writing to see `configErrors`.

## tasks: Running and stopping tasks

`task_start` takes an `idempotencyKey` you choose and either a `taskId` from
`sikemux.json` or a `command` to run. A `command` runs in the same kind of
managed terminal as a configured task, in the project root or in `cwd`, a
directory inside the project given relative to it, and `label` names its tab.
The result's `taskId` looks like `sh:pnpm-dev-3fa2c1`; use it with
`task_read`, `task_stop` and `task_restart`, or pass it back to `task_start`
to run the same command again. Starting the same command in the same
directory while it still runs returns that run. Use this for dev servers and
watchers rather than background shell jobs: you can read their output, see
when they exit, and they stop with the project.

The key is what makes a retry safe. Reusing a key returns the original
execution rather than starting a second one, and it keeps doing so after that
execution has finished. Pick one key per logical attempt and reuse it when a
call fails or you are unsure whether it landed.

Two things can stop a launch:

- A `sikemux.json` task may need trust. Sikemux shows its own dialog, and a
  changed `sikemux.json` is checked again before the next launch. While the
  dialog waits on the person, `task_start` returns at once with
  `status: "awaiting-trust"` and an `executionId` that `task_read` and
  `task_stop` already know. Call `task_start` again with the same key, or
  `events_wait` with that `executionId`, to see it start. If the person
  refuses, the run ends as `failed`. A `command` needs no trust, and neither
does anything you start while the person runs you in YOLO mode.
- If the same task is already running from the command deck, stop it there
  first. Starting a task already running through the harness just returns that
  execution.

`task_start` always answers within about 30 seconds. If the task is still
launching it returns `status: "starting"`; call again with the same key.
`readyWhen` makes it wait, within those 30 seconds, until that text appears
in the output, such as `Ready in` for a dev server. The result then carries
`ready`: `true` once it appeared, `false` if the task stopped or time ran out,
with a `note` when it is still running; keep waiting with `events_wait`, or
`task_read` with `search`.

`task_restart` takes a `taskId`, stops that task's latest execution, starts a
new one, and returns it. Use it instead of a stop followed by a start with a
new key.

A task you start opens its terminal as a tab on your desk, without taking the
person's focus. To bring the person to it, call `ui_open` with
`kind: "terminal"`, the `executionId`, and `focus: true`.

`task_stop` takes an `executionId` and stops that exact execution and
its process tree. A `taskId` instead stops that task's latest execution. It
does not stop a task started from the command deck.

### What survives a reload or restart

Tasks, their runs and idempotency keys live in Sikemux's background process,
so they survive the window reloading, the app quitting and Sikemux updating.
While the window is closed, `task_read`, `task_stop`, `events_wait` and
`task_start` with a key you already used keep working, and `workspace_inspect`
returns your runs and an event cursor with `window: null` and a `note`; its
panes, `sikemux.json` tasks and ports need the window. Starting a new run,
`ui_open` and the browser tools need the window too; with it closed they fail
and say so. While Sikemux updates its background process a call can fail for
a moment saying so; call it again. When Sikemux opens again the tasks'
terminals come back. Chat agents keep running too: a turn in progress goes
on while the window is closed, and a permission it asks for waits for the
person. A terminal agent whose process dies, other than by the person quitting
it or Sikemux stopping it, is started again on its saved conversation in the
same pane. A task that ended stays readable for about ten minutes.
Event cursors stay valid across reloads, restarts and updates, so keep the one
you have.

"Quit and Stop Everything" (⌥⌘Q) stops every task and the background process,
and its runs and keys go with it; `task_read` then says the task was started
earlier and its run is gone. Sikemux keeps 128 runs and 256 keys, forgetting
the oldest finished ones first; only when that many are still running does a
start fail, and stopping one with `task_stop` makes room. Closing the project
stops its harness tasks.

## output: Reading output and waiting

`task_read` pages through a task's output by byte cursor. Name the run with
its `executionId`, or pass a `taskId` to read that task's latest execution.

Start at cursor `0`. Pass the returned `cursor` into the next call. Keep
reading while `hasMore` is true. Every page reports `end`, the length of the
output so far. Pages are 8 KiB by default; `limit` accepts 4 to 8192 bytes.

Output is raw terminal data and may contain escape sequences. Three options
change what comes back:

- `plain: true` strips escape sequences and replays carriage returns and
  cursor moves, so a progress bar or build screen that redraws itself shows
  each line once, in its final state. Runs of an identical line or block of
  lines keep one copy and a note such as `[repeated 40 more times]`. The
  cursor still counts raw bytes, so plain pages chain like raw ones.
- `tail: N` returns the last N lines instead of paging from the cursor, which
  answers "what did the server just say". Its `cursor` is `end`, ready for the
  next read of new output.
- `search: "text"` returns only the lines containing that text, ignoring case,
  each with `context` lines around it (3 by default, at most 20), and groups
  separated by `--`. It reads plain text and reports `matches`. It pages
  forward from `cursor` like a normal read; with `tail: N` it returns the last
  N matches instead, such as the latest stack trace.

A task retains about 1 MiB. If your cursor is older than the retained bytes,
`truncated` comes back true — you have lost the gap and should read on from
the cursor you were given rather than trying to recover it.

Finished terminals are kept for roughly ten minutes, and can be dropped sooner
when the app is under pressure. Read output you care about promptly.

### Waiting for something to happen

`events_wait` blocks for up to 30 seconds and returns task output,
task lifecycle, and UI-open events.

**Event cursors are not output cursors.** They are separate sequences. Get an
event cursor from `workspace_inspect`, pass it to `events_wait`, and
pass each returned cursor into the next wait. Passing an output cursor here is
a mistake.

A timeout is normal: you get an empty event list and a fresh cursor. Wait
again. Pass an `executionId` to hear only about one task.

A wait reaches the project's newest 1024 events. If one comes back
`truncated`, stop replaying and inspect the workspace again for current state.

A wait does not schedule you a future turn. It only holds this call open.

## ui: Opening things for the person

`ui_open` takes a `kind`:

- `file` — with a `path` inside the project and an optional one-based `line`
- `diff` — with a `path`
- `terminal` — with an `executionId`
- `preview` — the project's configured preview

Files and terminals open on your desk, and the tab comes to the front there
without taking the person's focus. Add `focus: true` to bring the person to
your session as well; when they are working in another app, Sikemux only
bounces its Dock icon rather than taking their keyboard. A diff opens in the workspace.

Paths must resolve inside the project; a path that escapes it is refused.

Preview needs an agent session and opens as a page on your desk. The `previewUrl`
you get back is configuration, not proof that anything is listening. If you
need to know the server is up, read the task output or navigate to it.

## browser-reading: Reading a page

`browser_state` returns page state: url, title, numbered interactive elements,
visible text, and the open tabs. The text stops after about 2 KB, and
`textLength` then gives its full length; `fullText: true` returns up to 40 KB,
and a CSS `selector` returns only the text of each part it matches.

Tools that act (navigate, click, type with submit, press, drag, upload,
dialog, wait) report on the page afterwards, and `report`
chooses how much:

- `"changes"`, the default, returns url, title and loading plus `changes`
  since the last read of this page: `elements` lists new or changed elements,
  `removed` the numbers that went away, `textAdded` and `textRemoved` the lines
  of text that came and went. `changes: "none"` means nothing moved. When the
  page has not been read before, such as after a navigation, you get the full
  state instead.
- `"outcome"` returns only url, title, loading and what the action itself
  found, such as the clicked `label` and whether it was `covered`. Use it for a
  run of steps whose effect you will check afterwards.
- `"full"` returns the whole state, as `browser_state` would.

**Element numbers expire** when their element leaves the page. A number stays
with the same element for as long as that element is there, so a number from
an earlier read either reaches the same element or fails with "no element".
Elements that appear later get new numbers. A page that redraws a list builds
new elements, so read again after it does. Numbering starts again at 0 on
every new page, so never carry a number across a navigation; the error says
when a number was never handed out on this page.

Each element line shows what a person would see of its state: `value="…"` for
a filled field, `[checked]` or `[unchecked]` for checkboxes, radios and
switches (read from the hidden checkbox inside a styled switch too),
`[expanded]` or `[collapsed]`, `[selected]` and `[disabled]`.

The list leaves out elements a person cannot reach: ones lying under a
modal, a banner or an open menu, and ones the page marks `inert` or
`aria-hidden`. It also leaves out the parts of a listed control, such as a
label wrapping a listed checkbox or a clickable span inside a link. They keep
their numbers, so a number read earlier still works.

On a long page the list gives every element in view but only the 40 offscreen
elements nearest the view, marked `[offscreen]`; `offscreen` counts the rest
above and below. They keep their numbers, and `browser_find` finds them. A
change report lists such an element once it scrolls into view, and never
reports one you were not shown as removed.

`browser_find` with a `query` lists the elements whose visible text,
accessible name, placeholder, `name` or `id` contains it, with their numbers,
exact matches first. `role` narrows it, as in `button`, `link`, `checkbox`,
`textbox` or `tab`; when nothing of that role is named that way, it lists
every element of that role instead. When no element matches but the words
show on the page, it returns `points` with their `x`, `y` to click instead.

Controls inside a frame from the same site are numbered with the rest of the
page. A frame from another site, such as a card field or a sign-in widget,
cannot be read from outside, so state lists it as a single element. Click its
number, or better the field inside it by `x` and `y` from a screenshot, then
`browser_type` without an index to type at the caret. The page's text,
`browser_network` and `browser_console` cover only the top page.

## browser-input: Acting on a page

Clicks, keys and typing arrive as real input, the same as the person's, so
pages that check for a trusted event and editors that keep their own model of
the text both respond to them.

`browser_click` takes a number from the latest state, or `text` naming the
element by its visible text or accessible name, or a CSS `selector`, or `x`
and `y` in CSS pixels from the top left of the viewport, which is where a
screenshot's pixels sit too. `text` and `selector` must pick out one element:
when several match, nothing is clicked and the error lists them, so pass
their number, a `role` or a narrower selector. `expectLabel` makes the click
fail rather than land on an element whose label does not contain it, with
coordinates too. The result names the `index` and `label` it clicked; a click
by coordinates names what it `hit` instead, so check that before trusting a
point read off an older screenshot. Coordinates reach things that have no
number, such as a canvas or a field inside a frame from another site. `double: true` double-clicks.
`hover: true` only moves the pointer there, to open a hover menu; while
Sikemux is in the background the page is told about the hover but CSS
`:hover` styles do not apply. A result with `covered` names what was on top
of the element and took the click instead. `browser_navigate` with `go:
"back"` or `go: "forward"` moves through the current tab's history.

`browser_type` with an `index` or `selector` focuses that element and
replaces its value. Without one it types at the caret of whatever is focused,
so it adds to the text there; `replace: true` selects the focused field's
text first so the typing replaces it. `submit: true` presses Enter
afterwards. The result names the field it typed `into` and carries its
`value` afterwards (the end of it, with `valueLength`, when it is long), and a
`warning` when the value suggests the text did not land, such as a field
still empty. A `<select>` picks the option whose value or label matches the
text.

`browser_act` plays up to 20 `steps` in one call, each an `action` of
`click`, `type` or `press` with the same fields as that tool (`index`,
`text`, `role`, `selector`, `expectLabel`, `x`, `y`, `replace`, `submit`,
`key`). Use it to fill a
form and submit it, or to open a menu and pick from it by `text`. It stops at
the first step that fails, and after any step that navigates, opens a dialog
or changes tab, because the steps after it were planned for the old page.
`done` lists what each step did, `stopped` names the step it stopped at and
why, and `report` covers the page once at the end.

`browser_press` sends one key to the focused element: `Enter`, `Tab`,
`Escape`, `Backspace`, `Delete`, `ArrowDown`, `Home`, `PageDown`, `Space`, or
a single character. Hold modifiers with `+`, as in `Meta+a` to select all or
`Shift+Tab` to go back a field. `Meta` is the Command key. The editing chords
`Meta+a`, `Meta+c`, `Meta+x`, `Meta+v`, `Meta+z` and `Shift+Meta+z` act on the
page itself when it does not handle them; other command chords never reach
the Sikemux app.

`browser_drag` presses on `fromIndex` (or `fromX`, `fromY`), moves to
`toIndex` (or `toX`, `toY`) and releases there. Sliders and sortable lists
see a real pointer; items marked draggable get the page's own drag and drop
events.

`browser_upload` attaches files the way a person picking them would. Pass
absolute `paths` and the number (or `x`, `y`) of the file input or of the
button that opens its chooser. The chooser never shows; the page gets the
files and its change events. Only one file is given when the input takes one.

While you click, type or press, the tab holds the keyboard, and afterwards
the person's focus goes back to wherever they were typing, so they can keep
working in Sikemux while you drive a page. A menu that closes when its page
loses focus may therefore close between your calls; open it and pick from it
in one `browser_act`.

A page that hangs or whose process crashes is reported at once on the next
call: close that tab with `browser_close_tab` and open a new one.

`browser_dialog` answers an alert, confirm or prompt. Page state reports an
open one under `dialog`, and until it is answered the page is frozen: every
other browser tool refuses rather than hang. `accept: true` presses OK and
`false` presses Cancel; `text` fills a prompt first. The person sees the same
dialog and may answer it before you do.

`browser_scroll` with an `index`, `text` or `selector` alone brings that
element into view and says whether it is now on screen. `deltaY` moves by
that many pixels, negative for up, and `to: "top"` or `"bottom"` jumps to an
end; either acts on the page, or with a target on the scrolling container
around it. It returns the scroll position and `atBottom`.

## browser-pages: Pages, tabs and viewport

`browser_wait` waits for the page to reach a state. `text` or `textGone`
waits for words to show or go, `selector` or `selectorGone` for an element,
`url` for the address to contain a string, and `networkIdle: true` for the
page's fetch and XHR calls to have been quiet for half a second. Give several
and all must hold. It checks every 200 ms for up to `timeoutMs` (10 seconds by
default, 60 at most) and returns `met`, `waitedMs` and, when it ran out, the
conditions still `failing`, along with the page. With no condition it sleeps
for `ms` (default 1000, max 30000) and then waits for any load to finish.
Prefer a condition over a guessed sleep, and over screenshots taken to see
whether something has finished.

`browser_navigate` takes the same conditions as `waitFor`, to arrive at a
page that has finished drawing. Navigating to the url the tab is already on
reloads it; `go: "reload"` does so directly, and with it `hard: true` skips
the cache so a changed script or stylesheet is fetched
again. When the load failed, as when nothing listens on that port, the result
says so in `loadError`, and a page that loaded without any text or controls
yet comes with a note to wait for it.

Tabs only open `http` and `https` pages. To show a local HTML file, image or
folder, pass its absolute path, a `~/` path or a `file://` url to
`browser_navigate`: Sikemux serves its folder to the tab from a private
address on this machine, so relative links and assets load too.

To see Sikemux's own interface in a tab, as when checking a change to it,
run `pnpm showcase:serve` in the Sikemux repository and navigate to
`http://localhost:1471/showcase/`. It is the real frontend with hot reload
over demo data.

A tab's viewport follows the pane, so it changes size when the person resizes
the pane, and a tab the pane is not showing lays out at the size it last had.
`browser_viewport` holds the current tab at `width` and `height` in CSS pixels
(200 to 4000), or a `preset`: `desktop` is 1280×800, `tablet` 820×1180 and
`mobile` 390×844. `preset: "fit"` lets it follow the pane again. The size
stays through reloads and navigations in that tab. The page lays out at that
size and is scaled down to fit the pane, never up, so screenshots and `x`,
`y` stay in the viewport's CSS pixels. The `mobile` preset also sends
iPhone Safari's user agent and reloads the page, so sites that sniff it serve
their phone version; any other size sends desktop Safari's, as a real iPad
does. The pointer stays a mouse, so sites that check for touch still see none.
The full state reports `viewport` with the page's `width` and `height`, and
`fixed` holding the size you set or `false`, with a note when the pane is
narrow enough that the page may be showing its phone layout. A fixed size is
the page's real window size, so `vw` units, media queries and `innerWidth`
all see it, and the call returns once the page has laid out at it.

Every state carries `visible`: whether the person can see this tab in the
pane right now. A tab that is not visible still lays out at a real size and
takes input, but do not tell the person a page is on their screen when
`visible` is false. With no arguments it just returns the state.

Tabs are yours. `browser_state` lists them, even before any is open, and
`browser_switch_tab` and `browser_close_tab` act on this pane's tabs, not the person's other windows.
Switching returns what changed since you last read that tab, or its full
state when you have not read it yet.
`browser_navigate` reuses the current tab unless you pass `newTab: true`.
When an action makes the page open a tab of its own, as a link with a new
window target does, the result lists it under `openedTabs`, and that tab is
now the current one. Tabs do not survive a restart of Sikemux.

## browser-evidence: Screenshots, drawings and recordings

`browser_screenshot` returns an image of the visible part of the tab. Use it
when layout or rendering matters; use `browser_state` when you only need
text. Its pixels are CSS pixels, so a point you read off it can go straight to
`browser_click` as `x` and `y`. `fullPage: true` captures the whole page
instead, cut at 14,400 pixels tall (`cutAt` says when it was). An `index`,
`text` or `selector` captures only that element, scrolled into view, which
is the cheapest way to check one component.
`annotate: true` reads state afresh, draws each element's number on the
picture, and returns the list of the elements it drew, which is the quickest
way to match what you see to a number. It leaves out elements that are cut
off, covered, or behind an open modal, and boxes that would cover a large
share of the view.

While you act, the person sees a pointer move to each click, with a ripple
where it lands. Screenshots leave the pointer out. `browser_annotate` draws
for them on purpose: with `index` (or `x`, `y`) and `text` it boxes an element
with a label; with only `text` it shows a caption along the bottom of the page.
Drawings stay until `durationMs` passes or you call it with `clear: true`,
they follow the page as it scrolls, and screenshots include them. Use them to
point something out, or to narrate a recording.

`browser_record` with `action: "start"` films the tab you are on, ten frames a
second, until `action: "stop"`, which returns the video's `path`, length and
size. Pass an absolute `path` ending in `.mp4`, or it is saved in the
person's Movies folder under Sikemux, named after the page. It follows you
across tabs, includes your pointer, boxes and captions, and stops by itself
after ten minutes. Narrate with `browser_annotate` captions as you go; a
recording of a tab the person is not looking at still works.

## browser-debugging: Network, console and scripts

`browser_network` lists the fetch and XHR calls the page has made since it
loaded, oldest first, with each `id`, status, duration and the start of its
response body. Pass an `id` to read that one call whole.
It is how you tell a request that failed apart from a button that never asked,
which the DOM alone cannot show. A call that never got an answer carries the
`error` the page saw, such as `TypeError: Load failed`. Data requests a
framework makes behind a navigation are named in `framework`: `next rsc`,
`next server action` or `next data`. Narrow a busy page with `filter`, a
substring of the URL's path (or of the whole URL when no path matches),
`method` such as `POST`, and `status`: a code such as `404`, a class such as
`5xx`, `failed` or `pending`. A long list keeps the newest calls and says how
many it left out. Images, scripts and styles do not appear.

`documents` lists the tab's last 20 whole-page loads, oldest first, with each
`status`, `mimeType`, duration, and the `error` of a load that never reached
the server, which leaves no page behind to record anything. A load cut short
by the next one says `cancelled`. `since: "navigation"` keeps only the load
that produced the page on screen and any tried after it. A web app moving
between its own screens loads no document, so its calls stay listed with the
page that made them.

`browser_console` lists what the page logged since it loaded, oldest first:
each message's `level` (`log`, `info`, `warn`, `error`, `debug`, `uncaught`
for a thrown error nobody caught, `unhandled rejection` for a failed promise)
and its text. `errors: true` drops the log, info and debug noise. Up to 200
messages are kept, and the newest 50 are returned unless you pass `limit`.

`app_console` reads the Sikemux app's own window the same way, not a tab.
Its levels add `swallowed`, for errors the app caught and kept quiet, and
each message carries an ISO `at` time. Use it when Sikemux itself misbehaves,
such as a blank avatar or a panel that does not load, instead of asking the
person to open Web Inspector.

`browser_evaluate` runs JavaScript in the page and returns the result as
JSON. The code goes in `script`: an expression such as `document.title`, or
statements that end in `return`, since statements without one return
nothing. Promises are awaited for `timeoutMs`, 30 seconds by default and 60 at
most, and elements come back as their markup. Reach for it when no other tool
reads what you need. Prefer the other tools for acting, since they send real
input, and `browser_wait` for waiting, rather than a polling loop in a script.
A script that reloads or leaves the page loses its result; use
`browser_navigate` with `go: "reload"` for that. It runs with the page's own session, so `fetch` of
the site's API returns what the signed-in person would get.

## shell: The same operations from a shell

Every tool here is also a CLI verb, which is useful inside a task or a script:

```bash
sikemux tool workspace.inspect
sikemux tool task.start '{"taskId":"dev","idempotencyKey":"dev-first-run"}'
sikemux tool task.read '{"executionId":"ID","cursor":0}'
sikemux tool task.read '{"taskId":"dev","tail":50,"plain":true}'
sikemux tool events.wait '{"cursor":"CURSOR","timeoutMs":30000}'
sikemux tool ui.open '{"kind":"file","path":"src/App.tsx","line":42,"focus":true}'
sikemux tool task.stop '{"executionId":"ID"}'
```

`sikemux` is on PATH for the terminals, tasks and agents Sikemux launches,
including your own shell tool. Where it is not, `workspace_inspect` returns
its absolute path as `cli`.

Terminals that Sikemux launches already carry the project and agent context.
From any other shell, run the CLI inside the open project's Git root or set
`SIKEMUX_PROJECT`.
