# Notch island

The root `AGENTS.md` applies here too.

- The island over the MacBook notch is `sikemux-notch`, a SwiftUI helper in `helpers/notch`
  and a client of the core: it reads the device view phones get. The app starts it; it keeps
  running after the app quits.
- A new island state is mocked up in `helpers/notch/design/screens.html` before it is built;
  view it by serving the repo root (`python3 -m http.server`) and opening
  `/helpers/notch/design/screens.html`, after `node scripts/generate-notch-sources.mjs` has
  written the buddies it draws.
- Its icons come from `shared/ui/Icons.tsx`, the agent marks and `brand/`. Its Swift icons,
  settings, colours and buddies are build output, which `scripts/build-notch-helper.mjs`
  writes before it builds.
- Its agent rows are laid out like the app's tab rail rows (`app/src/agents/mod.rs`):
  the task with how long ago, the project, then the name, counts and state together at the end.
  When the rail's row changes, change the island's to match.
- `swift run --package-path helpers/notch notch-snapshots <dir>` renders every state to PNGs,
  for comparing with the design, once the helper's build has written its generated Swift.
