# ADR 0022: Plugins are compiled-in connectors, and mods are their panels

- Status: Accepted
- Decision date: 2026-11-01

## Context

[ADR 0002](./0002-plugins-are-compiled-in.md) made every integration a crate compiled into
the core, with a pane the app drew for it. Nobody used the panes: people reach these
services through agents. The panes, the code host's views of pull requests, issues, CI runs
and the inbox, and the top bar's CI chip came to about 35,000 lines, polled in the
background, and each had to be ported to GPUI.

## Decision

- **A plugin draws nothing in the app.** It is a connector: a sign-in in Settings →
  Connectors, the methods its manifest declares, and the agent tools it offers. The local
  Git pane stays, because it reads the repository rather than a code host.
- **A panel for a service is a mod.** A mod is a web page in the person's mods folder. It
  reads connector methods through the core's bridge (`sikemux-core/src/server/mods/bridge.rs`),
  and only those its `mod.json` declares. Writes are declared too and need the person's
  approval. Credentials stay in the core, so a page only ever sees answers.
- **There is no marketplace and no third-party plugin loading.** Every plugin is written in
  this repository, as a crate in `plugins/<name>` behind a Cargo feature of the same name on
  `sikemux-host`. People turn plugins off in the app.
- **A plugin crate depends on `sikemux-plugin-api` and, where it needs it, `sikemux-process`,
  never on the core.**
- **The release build keeps `panic = "abort"`.** `panic = "unwind"` would cost roughly 5–10%
  of the binary, and a process per plugin means shipping a second binary, which costs more
  than all the plugins together.

Crash containment comes from cheaper measures instead:

- Plugins run in the core's process on their own Tokio runtime
  (`crates/sikemux-host/src/plugins/host.rs`), so a stuck plugin cannot starve the terminals
  and agents.
- Every plugin call has a timeout: 60 seconds by default, which a plugin's manifest can set
  between 1 second and 10 minutes.
- Plugin crates deny `unwrap`, `expect`, `panic!`, unchecked indexing and string slicing
  through the workspace Clippy lints, so panics are hard to write in the first place.

## Consequences

- Adding a plugin means a pull request to this repository, not an install. Adding a panel
  needs neither: a person or their agent writes a mod, which adds nothing to the download.
- What a mod can show is limited to what connectors expose, so some `CodeHost` methods exist
  only for mods to reach.
- A panic in a plugin still aborts the core. The lints are the defence, so they must not be
  loosened for plugin crates.
- Plugins built mostly for size use `opt-level = "z"` in the release profile.
