# ADR 0020: The desktop and the web are one GPUI app that only draws what the core sends

- Status: Accepted
- Decision date: 2026-10-21

## Context

The desktop app is React in Tauri's WebKit. It grew one feature at a time into many thin
components, state copied between them and logic repeated beside the phone's. WebKit's memory
and main thread are what limit how many agents, panes and projects it keeps up with
(GitHub #115 holds the measurements and the choices behind this ADR). The same app is wanted
as the web client at app.sikemux.com.

## Decision

- **GPUI, on the `sonorahq/gpui` fork with GPUI Kit**, draws the Mac and the web (wasm) from
  one Rust UI. It is written fresh on the `gpui` branch and switched to once everything
  matches, through a pull request.
- **The core owns what the data means** ([ADR 0010](./0010-phone-links-only-the-wire-and-client.md)).
  The app draws the core's views: chats as rows and their changes, agents, questions, the
  catalog, and terminals as cell grids the core emulates.
- **One store.** The app holds one store of the core's view, entities fed by the core's
  changes, and a little state of its own: focus, open menus, scroll, sizes. No view keeps a
  copy of the core's data, no screen has a cache or a second path to the same fact, and what
  two views need lives in the store once.
- **Data flows one way.** The core's changes go into the store, the store notifies views,
  views draw. What the person does goes out as the core's `Call`s. No view writes another
  view's state.
- **A small shared component set comes first**: buttons, rows, list, tabs, menu, popover,
  input, pane chrome, icon and mark box, on GPUI Kit, themed from one token set taken from
  `DESIGN.md`. Screens are put together from these; a screen that needs a one-off widget
  shows the set is missing something, which is added there once.
- **Fewer, bigger pieces.** A few well-named components rather than many thin ones, no layers
  that only pass props through, no per-screen copy of the same row or header.
- **Smaller than what it replaces.** Each area's line count is reported against today's
  TypeScript and CSS for the same area, and is meant to be well below it. Each screen is
  checked against these rules, and simplified, before it lands.
- **Idiomatic GPUI, not the web app redrawn.** The app matches what people see and can do,
  never how the web app happens to build it.
  - It uses GPUI's own model: entities and views, actions with the keymap and key contexts
    for every shortcut, focus handles, GPUI's lists and scroll handles, its animation API,
    and its input events, including trackpad gestures and scroll phases.
  - Nothing GPUI or GPUI Kit already provides is rebuilt by hand. Nothing ports a web-ism:
    no emulated CSS cascade, no DOM tricks, no copy of the React component tree, no
    workaround that reproduces a WebKit quirk.
  - When matching a pixel of today's app would take a hack, the GPUI-natural form wins and
    its commit says so. `DESIGN.md`'s principles and tokens are the look to match, not
    today's exact rendering.
  - No feature is dropped. Swipes between spaces and tabs, every shortcut with today's
    command ids, drag and drop and the motion rules are all kept, done the GPUI way.
  - Each screen is reviewed for workarounds when it lands. A workaround already landed is
    made idiomatic the next time its area is touched, and the commit lists what changed.
  - A patch to the GPUI fork is a general capability GPUI lacks, fit to send upstream, such
    as the overlay plane or letter-spacing. It is never a Sikemux-specific hack.

## Consequences

- The behaviour and the look of the Tauri app are what is ported, not its components, its
  state or the way it is built.
- The React app keeps working on the branch until GPUI replaces it, drawing the same core
  views through `src/chat/rows.ts` and the generated wire types.
- Pieces nobody has built for GPUI yet become crates others can use, free of Sikemux types
  and `workspace-hack`, as #115 lists.
