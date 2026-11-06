# Sikemux v0.6.1-nightly.5

Nightlies are signed and delivered exactly like stable releases, but they carry unreleased work and can break. Switch back to stable in Settings → About whenever you want; you keep the build you are on until a stable release passes it.

## Before you update

- **Your phone needs 0.2.0-nightly.13 or later**, from TestFlight on iPhone or the Google Play testing track on Android.
- **This nightly links only with hosts on the 0.6.1 nightlies.** Hosts on stable 0.6.0 can't reach it.

## New since nightly.4

- **Home shows every goal and agent at a glance.** Six numbers rank the list, with filter tabs, a find box and rows under Needs you, Moving, Waiting, No goal and Done.
- **A chat can pick up a goal.** When a chat's work looks real, Sikemux offers Track as goal under the composer, and you can Undo it after. Its subagents count towards the goal.
- **Messages between agents fold to one line**, showing who sent them, how many, and what someone needs from you. Your last prompt stays in reach above them.
- **Agents settle more among themselves.** A worker's assumption stands, a blocked question goes back to its lead first, and the lead raises only what is yours to decide.
- **Let dev go unasked.** A held Rundeck deploy to a job in a folder that isn't production offers `Let dev/* go unasked`, so one click frees that whole folder without changing any slice.

## Fixes

- A wide character in a very narrow terminal can no longer end every agent on the host.
- A sent message no longer comes back into the composer and goes out again with the next one.
- A finished turn keeps its answer out of the fold when the agent wrote it before more calls and a short closing line.
- Claude Code's own questions get the answer you actually gave.
- A secret answer takes a multi-line paste, such as an env file or a private key.
- A chat starts even if its program was too busy to name its version in time, and a Muse or ACP turn the network cut short goes again when the network is back.
- The board and goals read again after a reconnect, and the Plan fold draws steps instead of raw JSON.
- Remotes on `ssh.bitbucket.org` work, ahead of Bitbucket's move on 2026-11-12.
- On the share page, pictures open when clicked, an agent's page shows as a card, and a shared chat wears its agent's mark.

Much of this nightly is a large internal restructuring of the app, the core and the phone's shared code. It should change nothing you see, so tell us if something that worked no longer does.

For the complete patch history, compare [`v0.6.1-nightly.4...v0.6.1-nightly.5`](https://github.com/nodelike/sikemux/compare/v0.6.1-nightly.4...v0.6.1-nightly.5).
