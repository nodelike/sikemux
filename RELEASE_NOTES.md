# Sikemux v0.5.1-nightly.4

The fourth nightly on the 0.5.1 line. Nightlies are signed and delivered exactly like stable releases, but they carry unreleased work and can break. Switch back to stable in Settings → About whenever you want; you keep the build you are on until a stable release passes it.

## New since nightly.3

- **The chat.** A finished answer folds its work above the reply, so the answer reads first. A reloaded chat keeps the real time of every turn, and live labels shimmer in place instead of sliding in.
- **What your agents cost.** The Activity page prices your agents' tokens and shows what they cost.
- **Bitbucket.** The Git pane shows whether a pull request can merge, Bitbucket's issues, its tags as releases and an inbox of pull requests waiting on you. Agents can read the issue tracker and an issue's comments.
- **Notifications.** Clicking an agent's notification opens that agent.
- **The installer.** The DMG opens in a branded window.
- Agents without YOLO open in safe mode while YOLO is on, and the agent picker drops its filler text.
- A blocked merge names the host it waits on, not always GitHub.
- The iOS Simulator says "shutting down" while a device winds down, and shows the real error if shutdown fails.
- A pane divider no longer keeps a focus ring after you drag it, and the terminal's icon font is smaller.

Thanks to Ankit Patidar for Bitbucket's issues, releases, inbox and merge status.

For the complete patch history, compare [`v0.5.1-nightly.3...v0.5.1-nightly.4`](https://github.com/nodelike/sikemux/compare/v0.5.1-nightly.3...v0.5.1-nightly.4).
