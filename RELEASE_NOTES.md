# Sikemux v0.6.1-nightly.1

The first nightly after 0.6.0. Nightlies are signed and delivered exactly like stable releases, but they carry unreleased work and can break. Switch back to stable in Settings → About whenever you want; you keep the build you are on until a stable release passes it.

## Before you update

- **Your phone needs 0.2.0-nightly.13**, from TestFlight on iPhone or the Google Play testing track on Android. It speaks the newer protocol this nightly does, so it connects to hosts on this nightly but not to hosts on stable 0.6.0.
- **This nightly links only with other hosts on it.** Hosts on stable 0.6.0 and hosts on this nightly can't reach each other.

## New since 0.6.0

- **Goals.** A view of its own, toggled from the title bar: Home lists what needs you in every project, grouped Decide, Unblock and Fix, and starts work from the chat's composer with Hand off, Plan first or Chat. A goal lists the promises its slices make, what each waits on and roughly how long, and the evidence that shows them.
- **Slices on evidence.** A board card that makes a promise passes Built, Looks right and Landed only on evidence Sikemux collects itself: a check on the owner's latest commit, a CI workflow, screenshots at desktop, phone and tablet widths. Auto's pull requests carry that evidence.
- **CI on the board.** A project's check can be its CI, and CI failing the same way twice in a row on the branch becomes a card.
- **Memory.** Sikemux keeps the rules you'd otherwise repeat in every chat, hands them to new chats and agents in your project, and offers to move each into the folder's AGENTS.md.
- **Waiting on you.** Any question or proposal an agent ends its turn with lands in the inbox, answerable in one click, and anything that comes to need you notifies you once.
- **Processes.** Sikemux's run-a-command tools are processes now, and an agent's runs tidy themselves: starting under the same label replaces the old run.
- **Connectors.** Settings › Connectors replaces Integrations, with each connector's mark and whether it is signed in.
- Agents' browser tabs tidy themselves, agent worktrees get branches named for their work, and the context ring counts how often a chat was compacted.
- Stopping everything puts chats to sleep and keeps them asleep until you open one, and `sikemux restart` keeps terminals and chats.
- The rail shows only your account's name, never its email, so screenshots don't reveal it.

For the complete patch history, compare [`v0.6.0...v0.6.1-nightly.1`](https://github.com/nodelike/sikemux/compare/v0.6.0...v0.6.1-nightly.1).
