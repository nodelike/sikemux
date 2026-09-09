# Sikemux v0.3.4

Sikemux 0.3.4 adds OMP and Grok as first-class coding agents, gives Markdown files a rendered preview, and lets you reveal hidden side rails without leaving a focused workspace.

## Agents and browser

- Run OMP and Grok in agent panes with resumable sessions, launch modes, native provider marks, and the same embedded-browser flow available to the other supported agents.
- Extend embedded-browser setup to Hermes, Pi, and OpenCode, including browser tools and the packaged runtime each CLI needs.
- Use Codex's indexed task titles, detect standard OpenCode installations, ignore empty Claude sessions, and survive Claude's self-update handoff without losing the launch.

## Editor and workspace

- Switch Markdown files between source and a styled rendered preview. GitHub Flavored Markdown tables, task lists, strikethrough, and links render in the preview, while raw HTML stays disabled.
- Reload clean editor buffers when their files change on disk while preserving unsaved buffers for conflict handling.
- The sessions rail and agent rail now sit together on the left, keeping project context beside agent activity while the center pane gets the remaining space.
- New Claude and Codex tabs now pick up their provider session title while the first turn is still running.
- Project folders marked "index itself" now appear in the session and project pickers, and open pickers refresh when project indexing settings change.
- Hover at either window edge to reveal a hidden sessions or agent rail. A revealed rail remains open while it contains keyboard focus, then closes when focus leaves.

## Git, updates, and Windows

- Load visible review diffs first, defer distant files more aggressively, and cap expensive token and word-diff work so large reviews respond sooner.
- Render updater release notes as Markdown inside the app.
- Resolve Windows agent profiles from their real home directories, detect the bundled Chromium runtime on Windows, and keep Unix-only PTY login checks out of ConPTY builds.

The published automatic-update feed continues to target Apple Silicon macOS.

For the complete patch history, compare [`v0.3.3...v0.3.4`](https://github.com/nodelike/sikemux/compare/v0.3.3...v0.3.4).
