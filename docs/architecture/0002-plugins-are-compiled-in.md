# ADR 0002: Plugins are first-party and compiled in

- Status: Accepted
- Decision date: 2026-09-24 (issue #6)

## Context

Sikemux ships integrations for AWS, Bitbucket, Bruno, GitHub, GitLab, Rundeck and SigNoz.
Issue #6 asked whether they should become loadable plugins, possibly from a
marketplace, and how a crashing plugin should be kept from taking the app down.

The options for crash isolation were measured against binary size, which is a
product goal (the README promises a download of about 10 MB):

- `panic = "unwind"` so a plugin panic could be caught costs roughly 5–10% of
  the binary.
- Running plugins in their own process means shipping a second binary, which
  costs more than all the plugins together.

## Decision

- There is no marketplace and no third-party plugin loading. Every plugin is
  written in this repository and compiled into the app.
- Each plugin is a Rust crate in `plugins/<name>` behind its own Cargo
  feature of the core (`sikemux-host`), and its pane in the app is drawn by
  `app/src/plugin_pane/<name>`. Users turn plugins on and
  off in the app.
- A plugin crate depends on `sikemux-plugin-api` and, where it needs them, the small
  `sikemux-process`, `sikemux-keychain` and `sikemux-loopback` crates, never on the
  core.
- Surface kinds are named `<plugin id>:<surface>`, for example
  `sikemux.rundeck:deploy`.
- The release build keeps `panic = "abort"`.

Crash containment comes from cheaper measures instead:

- Plugins run in the core's process on their own Tokio runtime
  (`crates/sikemux-host/src/plugins/host.rs`), so a stuck plugin cannot
  starve the terminals and agents.
- Every plugin call has a timeout: 60 seconds by default, which a plugin's
  manifest can set between 1 second and 10 minutes.
- Plugin crates deny `unwrap`, `expect`, `panic!`, unchecked indexing and string
  slicing through the workspace Clippy lints, so panics are hard to write in the
  first place.

## Consequences

- Adding a plugin means a pull request to this repository, not an install.
- A panic in a plugin still aborts the core. The lints are the defence, so they
  must not be loosened for plugin crates.
- Plugins built mostly for size use `opt-level = "z"` in the release profile.
