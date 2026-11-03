# Sikemux v0.6.1-nightly.2

Nightlies are signed and delivered exactly like stable releases, but they carry unreleased work and can break. Switch back to stable in Settings → About whenever you want; you keep the build you are on until a stable release passes it.

## Before you update

- **Your phone needs 0.2.0-nightly.13 or later**, from TestFlight on iPhone or the Google Play testing track on Android.
- **This nightly links only with hosts on the 0.6.1 nightlies.** Hosts on stable 0.6.0 can't reach it.

## New since nightly.1

- **Mods.** Panels you or an agent make as a web page, kept in `~/.config/sikemux/mods/`. They open as tabs, read what their `mod.json` declares, and write only after you allow it. The side rail's Mods group lists them with a + that asks an agent for a new one, and Sikemux ships a Pull requests example to copy and change.
- **Several questions in one reply.** A reply that asks several things now makes a question of each, one tab apiece, and anything a yes or a no answers takes one tap. The answers reach the agent together, each beside its question.
- **Answers are never lost.** If you write to an agent before answering everything it asked, the answers you gave go with your message and show under it as chips.
- **Goals act like the workbench.** Anything you open from Goals now shows where you can see it, instead of behind the goals. Right-click a line about an agent for its menu, and a goal's lead, a slice's owner, tickets, files and commits all open what they name.
- **Home's start box is the chat's composer.** Drop files anywhere on Home and use the @ # ! and + menus and dictation. Your draft survives a restart.
- A terminal opened from the side rail's Terminals group lists there again.

For the complete patch history, compare [`v0.6.1-nightly.1...v0.6.1-nightly.2`](https://github.com/nodelike/sikemux/compare/v0.6.1-nightly.1...v0.6.1-nightly.2).
