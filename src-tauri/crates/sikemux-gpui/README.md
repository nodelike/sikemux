# sikemux-gpui

The desktop UI on GPUI (issue #115), a client of the core. It draws only what
the core sends.

## Run

```sh
node icons.mjs     # after an icon in src/ui/Icons.tsx changes
python3 fonts.py   # after the mono font in public/fonts changes (needs fontTools, brotli)
cargo build --manifest-path ../../Cargo.toml -p sikemux-host --target-dir target
cargo run
```

The app starts its core, or moves the one running onto its own build, as the
Tauri app does. A dev core takes requests only from the program named
`sikemux` beside it, so the core is built into this crate's target folder.

GPUI comes from the `sikemux` branch of sikemux/gpui, our fork of sonorahq/gpui,
pinned by commit in `[patch.crates-io]`. That branch is upstream plus our
commits and a commit that renames the crates to the gpui-pre names and version
GPUI Kit pins; commits made since sit on top of it, so the pin keeps working.
To move onto a newer upstream, rebase our commits, drop the rename commit and
run `node tooling/gpui-pre-names.mjs <version>` again.

It talks to the core at `SIKEMUX_CORE_SOCKET`, else `~/.config/sikemux/core.dev.sock`
in a dev build and `core.sock` in a release one.
It is a workspace of its own, so GPUI stays out of the app's lockfile and
workspace-hack.

## Rules

- Built from scratch and lean. Port the Tauri app's behaviour and look, never
  its structure.
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
- Line counts stay well below today's TypeScript and CSS for the same area.
- Before a screen lands, check it against these rules and simplify it.
