# sikemux-gpui

The desktop UI on GPUI (issue #115), a client of the core. It draws only what
the core sends.

## Run

```sh
node icons.mjs     # after an icon in shared/ui/Icons.tsx changes
python3 fonts.py   # after the mono font in shared/fonts changes (needs fontTools, brotli)
cargo build -p sikemux-host
cargo run -p sikemux-gpui
```

The app starts its core, or moves the one running onto its own build. `make dev`
does all of this and opens the app as Sikemux Dev. A dev core takes requests
only from the program named `sikemux` beside it, and both build into the
workspace's one target folder.

GPUI comes from sikemux/gpui, our fork of sonorahq/gpui, pinned by commit in
the root `Cargo.toml`'s `[patch.crates-io]` (today the head of its
`sikemux-zed-08cbb88` branch). That branch is upstream plus our commits and a
last commit that renames the crates to the gpui-pre names and version GPUI Kit
pins. To move onto a newer upstream, replay our commits, drop the rename commit
and run the fork's `node tooling/gpui-pre-names.mjs <version>` again.

It talks to the core at `SIKEMUX_CORE_SOCKET`, else `~/.config/sikemux/core.dev.sock`
in a dev build and `core.sock` in a release one.

## Rules

- Built from scratch and lean.
- One source of truth. The core owns the data. The UI keeps one store of the
  core's view, fed by the core's changes, plus UI-only state: focus, open menus,
  scroll, sizes. No view keeps a copy of core data or a cache of its own, and
  no fact reaches a view by two paths.
- One-way data flow. Core changes go into the store, the store notifies views,
  views render. What the person does goes out as core calls. No view writes
  another view's state.
- A small shared component set first, on GPUI Kit and themed from one token
  set taken from DESIGN.md: buttons, rows, list, tabs, menu, popover, input,
  pane chrome, icon, mark box. Screens are built from these. A one-off widget
  means the set is missing something: add it there, once.
- Fewer, bigger pieces. No pass-through layers, and no per-screen copies of the
  same row or header.
- Before a screen lands, check it against these rules and simplify it.
