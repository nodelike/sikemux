# Sikemux v0.5.0

Sikemux 0.5.0 brings your agents to your phone through a Sikemux account, puts them in the island over the notch, lets you keep more than one Claude or Codex account, and makes agents work out of the box on Macs whose shell starts slowly.

## Your account and your phone

- Sign in to a Sikemux account from Settings → Devices, on app.sikemux.com. This computer becomes a host on your account, and your picture shows at the right of the top bar.
- Phones signed in to the same account ask to connect, and you allow them once, with full control or to watch. The request takes the whole window, or opens the notch island when Sikemux is in the background. Pairing codes are gone, and remote access is on while you are signed in.
- When you are away, a paired phone is told when an agent asks for permission, needs input, finishes or hits a problem, and can answer from the notification. Each notification is encrypted for that phone alone.
- Phones open long chats at their latest turns and load the rest as you scroll, resume your recent chats, send photos and files, steer a running turn, stop tasks and switch YOLO.
- When your phone cannot reach this computer directly, it goes through Sikemux's own relay, still encrypted end to end. Remove a phone at app.sikemux.com and this computer forgets it at once.

## The island over the notch

- A small island over the notch shows your agents with a mark and a count for each state they are in. Open it to see each agent, what it is doing and its running subagents, and pick one to jump to it. A Mac without a notch gets a pill at the menu bar's height. Settings → Notch picks the displays and how it opens.

## Agent accounts

- Keep more than one Claude or Codex account in Settings → Agents: who each is signed in as and its plan, added and signed in and out through the agent's own login. Sikemux never reads a credential.
- The account card in the agent rail shows how much of its limits the account in use has left, and switches which one new chats start on.
- With two or more accounts, a chat that hits a usage limit can carry on with the next signed-in one. It is off by default; turn it on in Settings.
- Signing in again in a terminal reaches chats that are already open, and a turn that needs a sign-in offers one, then sends your message again.

## Agents that work out of the box

- Chats no longer say "Authentication required" while Claude works in your terminal, and SSH hosts no longer report `ssh` as not found. Both happened on Macs whose shell took a few seconds to start; a slow shell profile now costs only those seconds.
- Chats get what your shell exports, such as an API key or a `CLAUDE_CONFIG_DIR` in `.zshrc`, when Sikemux is opened from the Dock.
- Node from nvm, Volta, mise, asdf and fnm is found, Claude chats run on a Node new enough for them, and a failed install says why.
- Git and the other tools Sikemux runs use your shell's SSH agent and locale, so pushing through 1Password or Secretive works as in a terminal.

## Jira

- A Jira Cloud plugin: search, read, comment on, move, assign, create and log time on issues. The branch's issue shows in the top bar, and agents get the same as tools.

## Chat, desk and rails

- A floating header: the chat's title and controls sit in pills at the top, with the transcript running up beneath them.
- The desk opens and closes as a drawer, and shows one kind of tab at a time: pages, files or terminals. A new browser tab opens ready to type an address.
- The rails slide like drawers, and hiding both is focus mode. Double-click a tab to rename it.
- Tool calls read as what they did, keep their targets once finished, and show the plugin or service they belong to.
- Voice captions float above the composer instead of covering its buttons.

## Everything else

- Downloads are notarized, so a fresh download opens without removing quarantine first.
- Browser tabs and the window draw at your display's full refresh rate, up to 120 frames a second, and swiping between screens no longer stalls or flashes.
- Ports show one line each, Git opens on the history, agents list most recently active first, and the notch, chat and rail use less CPU while agents work.

For the complete patch history, compare [`v0.4.3...v0.5.0`](https://github.com/nodelike/sikemux/compare/v0.4.3...v0.5.0).
