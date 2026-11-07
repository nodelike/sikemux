# Sikemux v0.6.1-nightly.6

Nightlies are signed and delivered exactly like stable releases, but they carry unreleased work and can break. Switch back to stable in Settings → About whenever you want; you keep the build you are on until a stable release passes it.

## Before you update

- **Your phone needs 0.2.0-nightly.13 or later**, from TestFlight on iPhone or the Google Play testing track on Android.
- **This nightly links only with hosts on the 0.6.1 nightlies.** Hosts on stable 0.6.0 can't reach it.

## New since nightly.5

- **Commands as evidence.** An assertion can be shown by a command, so agents can write gates for tools no connector covers. An agent's command runs only once you accept that exact command under Needs you. It then runs on a slice's latest commit in a clean checkout of its own, and its exit code is the verdict.
- **Accepting a command accepts its files.** If you accept `sh ./verify.sh` and the script later changes, the command is held, so a slice can't pass by editing what judges it.

## Fixes

- A question ending "Is that okay?" or "Thoughts?" now shows the real question it stands for, so you know what you're answering.
- A deleted chat stops asking to be resumed, and goals stalled long ago clear once rather than coming up with timers like 364 hours.
- Home's badge and a goal's rail count clear as soon as a command is answered.
- A project's check runs its generate command once you trust its `sikemux.json`.
- A sleeping chat keeps its rows when you write to it.
- An agent you set going again can wake a teammate held by Stop Everything.
- Environments sits under its project in the tree, and its page names the project.
- The Mac installer finds Apple Silicon under Rosetta, and says so before replacing a nightly with stable.

For the complete patch history, compare [`v0.6.1-nightly.5...v0.6.1-nightly.6`](https://github.com/nodelike/sikemux/compare/v0.6.1-nightly.5...v0.6.1-nightly.6).
