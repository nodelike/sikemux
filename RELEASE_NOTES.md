# Sikemux v0.6.1-nightly.19

Nightlies are signed and delivered exactly like stable releases, but they carry unreleased work and can break. Switch back to stable in Settings → About whenever you want; you keep the build you are on until a stable release passes it.

## Before you update

- **Your phone needs 0.2.0-nightly.13 or later**, from TestFlight on iPhone or the Google Play testing track on Android. Phone 0.2.0-nightly.24 is out now.
- **This nightly links only with hosts on the 0.6.1 nightlies.** Hosts on stable 0.6.0 can't reach it.

## New since nightly.17

- **Eight languages.** The Mac app and both phones speak eight languages, and Settings search finds the current language's words.
- **Try Sikemux free.** OpenCode with no sign-in is ready on its free models, so you can try Sikemux with no subscription, and a first run with an agent already signed in opens straight on your newest project.
- **Auto permission mode** sits between Normal and YOLO: Claude Code and Codex review risky actions themselves.
- **Decisions show where they were made.** A decision an agent records itself shows under its reply as "Decided", with what it rests on and Overrule.
- **The chief.** It greets you once, shows what happened while you were away, and a call's card says what its asker goes on assuming until you answer, with Go with this.
- **Prompt cache.** The context meter fills toward where the agent compacts, and shows when a big Claude chat's prompt cache has expired, with a choice to summarise first.
- **Heavy commands wait their turn.** Agents' heavy commands wait in one line on the host, admitted by measured memory, and `sikemux.json` can list your own.
- **Agents on linked hosts** act like local ones: menus, pausing, renaming, closing, and what they wait on you for.
- **1Password.** Keys and tokens can be `op://` references; Sikemux keeps only the reference.
- **Search a host's chats by what was said**, on both phones.
- **Usage** shows cache writes as their own figure and where each model's cost went.
- **A new model menu.** Agents down the left, their models in the middle with search and ⌘1–⌘4, effort on the right; each model wears its provider's logo.
- **Safe, Auto and YOLO** each show their icon and a line on what they do, and a new chat's mode works before its agent starts.
- **"I don't have one."** A call for a secret can be answered that you have none, on the Mac, both phones and the notch.
- **Agents in a terminal tab can report their own state** (working, blocked, done) with OSC 7501, the Program Status Protocol.
- **Pane picture.** A new chat's welcome shows your pane picture over the whole pane, its dot size takes hundredths, and its dither controls have Reset.
- Every UI icon is now Lucide's.

## Fixes

- **What you type for an access call stays private**, as with a secret: it reaches no agent and no record, the keyboard learns nothing from it, and words kept from earlier versions are scrubbed when you update. The same holds when a worker asks its lead for access.
- A chat closed, or asleep when the core restarts, comes back with every row Sikemux added to it, such as Decided lines, each once, plus turns typed in a terminal since.
- A file an agent's quick shell command wrote is credited to that agent, so its turn no longer reads "no changes".

For the complete patch history, compare [`v0.6.1-nightly.17...v0.6.1-nightly.19`](https://github.com/nodelike/sikemux/compare/v0.6.1-nightly.17...v0.6.1-nightly.19).
