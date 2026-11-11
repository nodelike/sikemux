# Sikemux v0.6.1-nightly.14

Nightlies are signed and delivered exactly like stable releases, but they carry unreleased work and can break. Switch back to stable in Settings → About whenever you want; you keep the build you are on until a stable release passes it.

## Before you update

- **Your phone needs 0.2.0-nightly.13 or later**, from TestFlight on iPhone or the Google Play testing track on Android.
- **This nightly links only with hosts on the 0.6.1 nightlies.** Hosts on stable 0.6.0 can't reach it.

## New since nightly.13

- **A redesigned chat.** Replies read like an article, your messages are clean bubbles that stack, and thinking reads upright in a quieter voice. A run of tool calls is one flat strip that says what it did, such as "Edited 1 file, ran tests · 1 failed", and a code block is one card.
- **Conversations read as conversations.** Answered questions sit in the asker's bubble with your answer in yours, and a teammate's message or a trigger that fired gets its own bubble at the left.
- **The line under a running turn says what the agent is on**, such as "Running cargo test" or "Waiting for @Kuro".
- **Diffs and command output read quietly**, with line numbers and a +/− gutter, and subagents and goal milestones get their own lines.
- **The composer holds only what you type.** Where the chat runs, the host, project, branch, goal and account, sits quietly under it.
- **Board goals are lists**, one row per card, grouped Needs you, In progress, Open and Done.
- **Limit resets.** Each account shows its 5-hour session over its week, and a Claude or Codex limit reset can be spent from the limits float.

## Fixes

- The Linux app reaches its linked hosts.
- Crash reports send each run's session when it starts, and the notch starts and ends its own.
- A Claude account always shows its 5-hour window, at 0% until a session starts one.

For the complete patch history, compare [`v0.6.1-nightly.13...v0.6.1-nightly.14`](https://github.com/nodelike/sikemux/compare/v0.6.1-nightly.13...v0.6.1-nightly.14).
