---
name: sikemux-harness
description: How to drive a Sikemux project and its browser — task launches, output and event cursors, opening UI, and the tab model.
---

# Working inside Sikemux

You are running in a pane of a Sikemux workspace. The tools named `sikemux_*`
act on the project the person has open. The tools named `browser_*` act on the
browser tabs in your own pane, which the person can see.

Read this once before your first task launch or browser click. Everything the
tool descriptions leave out is here.

## Start by inspecting

`workspace_inspect` is the entry point. It returns the open project,
its panes, the tasks configured in `sikemux.json`, the harness runs you already
started, and an event cursor. Task ids come from there — do not guess one.

## Running a task

`task_start` takes a `taskId` and an `idempotencyKey` you choose.

The key is what makes a retry safe. Reusing a key returns the original
execution rather than starting a second one, and it keeps doing so after that
execution has finished. Pick one key per logical attempt and reuse it when a
call fails or you are unsure whether it landed.

Two things can stop a launch:

- The project may need trust. Sikemux shows its own dialog, and a changed
  `sikemux.json` is checked again before the next launch.
- If the same task is already running from the command deck, stop it there
  first. Starting a task already running through the harness just returns that
  execution.

Task terminals open in the background. To show one to the person, call
`ui_open` with `kind: "terminal"`, the `executionId`, and `focus: true`.

## Reading output

`task_read` pages through a task's output by byte cursor.

Start at cursor `0`. Pass the returned `cursor` into the next call. Keep
reading while `hasMore` is true. Pages are 8 KiB by default; `limit` accepts
4 to 8192 bytes.

Two things to expect in the bytes:

- Output is raw terminal data and may contain escape sequences.
- A task retains about 1 MiB. If your cursor is older than the retained bytes,
  `truncated` comes back true — you have lost the gap and should read on from
  the cursor you were given rather than trying to recover it.

Finished terminals are kept for roughly ten minutes, and can be dropped sooner
when the app is under pressure. Read output you care about promptly.

## Waiting for something to happen

`events_wait` blocks for up to 30 seconds and returns task output,
task lifecycle, and UI-open events.

**Event cursors are not output cursors.** They are separate sequences. Get an
event cursor from `workspace_inspect`, pass it to `events_wait`, and
pass each returned cursor into the next wait. Passing an output cursor here is
a mistake.

A timeout is normal: you get an empty event list and a fresh cursor. Wait
again. Pass an `executionId` to hear only about one task.

Event history holds 256 entries. If a wait comes back `truncated`, stop
replaying and inspect the workspace again for current state.

A wait does not schedule you a future turn. It only holds this call open.

## Opening things for the person

`ui_open` takes a `kind`:

- `file` — with a `path` inside the project and an optional one-based `line`
- `diff` — with a `path`
- `terminal` — with an `executionId`
- `preview` — the project's configured preview

Files open as background tabs. Add `focus: true` to actually reveal one.

Paths must resolve inside the project; a path that escapes it is refused.

Preview needs an agent session and opens in your own browser. The `previewUrl`
you get back is configuration, not proof that anything is listening. If you
need to know the server is up, read the task output or navigate to it.

## Stopping

`task_stop` takes an `executionId` and stops that exact execution and
its process tree. It does not stop a task started from the command deck.

## The browser

`browser_state` returns page state: url, title, numbered interactive elements,
visible text, and the open tabs. Most browser tools return the same shape after
acting, so you rarely need a separate read.

**Element numbers expire.** They are only valid until the next state read. Read
state, act on a number from that read, and treat the numbers in the result as
the new set. Never reuse a number across two reads.

Clicks, keys and typing arrive as real input, the same as the person's, so
pages that check for a trusted event and editors that keep their own model of
the text both respond to them.

`browser_click` takes a number from the latest state, or `x` and `y` in CSS
pixels from the top left of the viewport, which is where a screenshot's pixels
sit too. Coordinates reach things that have no number, such as a canvas or a
field inside a frame from another site. `double: true` double-clicks.
`hover: true` only moves the pointer there, to open a hover menu; while
Sikemux is in the background the page is told about the hover but CSS
`:hover` styles do not apply. A result with `covered` names what was on top
of the element and took the click instead. `browser_back` and
`browser_forward` move through the current tab's history.

Controls inside a frame from the same site are numbered with the rest of the
page. A frame from another site, such as a card field or a sign-in widget,
cannot be read from outside, so state lists it as a single element. Click its
number, or better the field inside it by `x` and `y` from a screenshot, then
`browser_type` without an index to type at the caret. The page's text,
`browser_extract`, `browser_network` and `browser_console` cover only the
top page.

`browser_type` with an `index` focuses that element and replaces its value.
Without one it types at the caret of whatever is focused, so it can add to
text rather than replace it. `submit: true` presses Enter afterwards. The
result carries the field's `value` afterwards, so you can check it took. A
`<select>` picks the option whose value or label matches the text.

`browser_press` sends one key to the focused element: `Enter`, `Tab`,
`Escape`, `Backspace`, `Delete`, `ArrowDown`, `Home`, `PageDown`, `Space`, or
a single character. Hold modifiers with `+`, as in `Meta+a` to select all or
`Shift+Tab` to go back a field. `Meta` is the Command key.

`browser_drag` presses on `fromIndex` (or `fromX`, `fromY`), moves to
`toIndex` (or `toX`, `toY`) and releases there. Sliders and sortable lists
see a real pointer; items marked draggable get the page's own drag and drop
events.

`browser_upload` attaches files the way a person picking them would. Pass
absolute `paths` and the number (or `x`, `y`) of the file input or of the
button that opens its chooser. The chooser never shows; the page gets the
files and its change events. Only one file is given when the input takes one.

`browser_dialog` answers an alert, confirm or prompt. Page state reports an
open one under `dialog`, and until it is answered the page is frozen: every
other browser tool refuses rather than hang. `accept: true` presses OK and
`false` presses Cancel; `text` fills a prompt first. The person sees the same
dialog and may answer it before you do.

`browser_evaluate` runs JavaScript in the page and returns the result as
JSON. Pass an expression such as `document.title`, or a function body that
uses `return`. Promises are awaited for up to 30 seconds, and elements come
back as their markup. Reach for it when no other tool reads what you need;
prefer the other tools for acting, since they send real input.

`browser_scroll` moves the page by `deltaY` pixels, default 600, negative for
up. Pass an `index` to scroll inside a scrollable element instead.

`browser_wait` sleeps for `ms` (default 1000, max 30000) and then waits for any
load to finish. Prefer it over repeated state reads when a page is settling.

`browser_screenshot` returns an image of the visible part of the tab. Use it
when layout or rendering matters; use `browser_extract` when you only need
text. Its pixels are CSS pixels, so a point you read off it can go straight to
`browser_click` as `x` and `y`. `fullPage: true` captures the whole page
instead, cut at 14,400 pixels tall (`cutAt` says when it was).
`annotate: true` reads state afresh, draws each element's number on the
picture, and returns the element list with it, which is the quickest way to
match what you see to a number.

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

`browser_network` lists the fetch and XHR calls the page has made since it
loaded, oldest first, with each status, duration and a truncated response body.
It is how you tell a request that failed apart from a button that never asked,
which the DOM alone cannot show. Narrow a busy page with `filter`, a substring
of the URL. Only fetch and XHR appear; images, scripts and the document itself
do not.

`browser_console` lists what the page logged since it loaded, oldest first:
each message's `level` (`log`, `info`, `warn`, `error`, `debug`, `uncaught`
for a thrown error nobody caught, `unhandled rejection` for a failed promise)
and its text. `errors: true` drops the log, info and debug noise. Up to 200
messages are kept, and the newest 50 are returned unless you pass `limit`.

Tabs are yours. `browser_list_tabs`, `browser_switch_tab` and
`browser_close_tab` act on this pane's tabs, not the person's other windows.
`browser_navigate` reuses the current tab unless you pass `newTab: true`.

## What does not survive

Run history and idempotency keys live for the current frontend session, capped
at 128 runs and 256 keys. Restarting the app does not resume commands, and
reloading the frontend loses run handles. Closing the project stops its harness
tasks.

If you hit a capacity error on either cap, the person needs to restart Sikemux;
you cannot clear it yourself.

## The same operations from a shell

Every tool here is also a CLI verb, which is useful inside a task or a script:

```bash
sikemux tool workspace.inspect
sikemux tool task.start '{"taskId":"dev","idempotencyKey":"dev-first-run"}'
sikemux tool task.read '{"executionId":"ID","cursor":0}'
sikemux tool events.wait '{"cursor":"CURSOR","timeoutMs":30000}'
sikemux tool ui.open '{"kind":"file","path":"src/App.tsx","line":42,"focus":true}'
sikemux tool task.stop '{"executionId":"ID"}'
```

Terminals that Sikemux launches already carry the project and agent context.
From any other shell, run the CLI inside the open project's Git root or set
`SIKEMUX_PROJECT`.
