# Sikemux v0.6.1-nightly.22

Nightlies are signed and delivered exactly like stable releases, but they carry unreleased work and can break. Switch back to stable in Settings → About whenever you want; you keep the build you are on until a stable release passes it.

## Before you update

- **Your phone needs 0.2.0-nightly.13 or later**, from TestFlight on iPhone or the Google Play testing track on Android.
- **This nightly links only with hosts on the 0.6.1 nightlies.** Hosts on stable 0.6.0 can't reach it.

## New since nightly.20

- **Agents show as chips in the chief's turn line and in an agent's header.** "Waiting on @Mujika" and the lead crumbs open the agent you click.
- **A chief's agents are root rows in the rail**, under the chief's own row, with no indent.
- **A chat coming to the front puts the keys in its composer**, so you can type straight away after opening, resuming or picking a chat.
- **A project with nothing open opens on a new chat** instead of an empty stage.
- **Agents can start teammates without limits** on how deep or how many, and the agent guide is half as long.

## Fixes

- A project's chief can close any finished agent in its project, not only the ones it started.
- Browser tabs an agent opens lay out at desktop size from their first frame, not at the width of the narrow desk pane.
- A push from another repository no longer asks to change this project's Stable releases, and each GitHub write's call says what it writes.
- Landing an agent's work runs Clippy only on the crates it changes and leaves the tests to CI, so lands go through much faster.
- Only places known to be production hold an agent's action for you; a name that says nothing no longer does.
- An agent's desk shuts when its last tab closes, even while its tab is out of sight.
- `git_commit` no longer refuses a commit over a missing import or a line crediting the model; it warns or drops the line instead.
- The daily usage ping counts each agent's chats, terminals, turns and time.

For the complete patch history, compare [`v0.6.1-nightly.20...v0.6.1-nightly.22`](https://github.com/nodelike/sikemux/compare/v0.6.1-nightly.20...v0.6.1-nightly.22).
