# Notch island

The root `AGENTS.md` applies here too.

- The island over the MacBook notch is `sikemux-notch`, a SwiftUI helper in `helpers/notch`
  and a client of the core: it reads the device view phones get. The app starts it; it keeps
  running after the app quits.
- A new island state is mocked up in `helpers/notch/design/screens.html` before it is built;
  view it by serving the repo root (`python3 -m http.server`) and opening
  `/helpers/notch/design/screens.html`.
- Its icons come from `shared/ui/Icons.tsx`, the agent marks and `brand/`. After changing one it
  uses, run `node scripts/generate-notch-icons.mjs`; the release checks fail on a stale copy.
- Its agent rows are laid out like the app's tab rail rows (`app/src/agents/mod.rs`):
  the task with how long ago, the project, then the name, counts and state together at the end.
  When the rail's row changes, change the island's to match.
- `swift run --package-path helpers/notch notch-snapshots <dir>` renders every state to PNGs,
  for comparing with the design.
