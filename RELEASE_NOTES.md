# Sikemux v0.4.0-nightly.10

The tenth nightly build. Nightlies are signed and delivered exactly like stable releases, but they carry unreleased work and can break. Switch back to stable in Settings → About whenever you want; you keep the build you are on until a stable release passes it.

## The rail goes back to two

The single rail that arrived in nightly.9 is reverted: the agents have a rail of their own again. If you updated to nightly.9 and lost the agent rail, this brings it back.

## The browser

- An agent can see what a page asked the server for.
- The address bar follows a page that moves on its own, rather than showing where the last full load landed — a site that navigates without fetching a new document no longer leaves the bar behind.
- Only a screen that paints may carry its page, so a tab off stage stops holding one.

## Elsewhere

- WebGL stops squeezing the terminal's columns together.
- The tab strips go with the window.

For the complete patch history, compare [`v0.4.0-nightly.9...v0.4.0-nightly.10`](https://github.com/nodelike/sikemux/compare/v0.4.0-nightly.9...v0.4.0-nightly.10).
