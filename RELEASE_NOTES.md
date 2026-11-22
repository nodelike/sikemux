# Sikemux v0.6.3

About 260 changes after 0.6.2. Google's Antigravity joins the agents, Settings is one sheet, a chat can hand its context to another model, and Sikemux keeps files it can't read instead of writing over them.

## What's new

- **Antigravity.** Google's Antigravity runs as a chat agent, signed in with your Google account, with its own mark and its own usage limits under LIMITS.
- **One-sheet Settings.** Eleven entries on one sheet. One Agents roster holds installs, hosts and accounts, Appearance shows pictures first with finer controls folded away, and Language moves to General.
- **Switch models mid-chat.** An agent can move a chat to another model or CLI, and a divider marks where the context was handed over. Each model in the picker shows its maker's mark.
- **Share links.** A shared link opens for anyone, no sign-in needed, and the Usage page has a card to share your week with its split by model.
- **Mods from GitHub.** Install a mod with a `sikemux://install-mod` link.
- **Calls.** ⌘⏎ keeps a call open and ⇧⏎ adds a line. A long choice stays inside its button.
- **Lead chats.** A lead's chat no longer shows a row for every finished worker, and a subagent shows as one row with its model in both the transcript and the dock.
- **The notch.** The island opens on your agents, with calls folded to one line.
- **Org vaults.** An org owner or admin can make a shut vault again from Settings, which names who holds its key.
- **Updates.** An update no longer waits for agents to finish their turn. A Mac app run from its disk image says to move it to Applications first.
- **Teams.** An organisation's key and models are offered only in projects shared with it.
- **Leaner top bar.** The usage chip and its popover are gone from the top bar.

Also fixed: a file Sikemux can't read is set aside and named, never written over; a closed agent keeps its name; coming back to a project opens the copy you had in front; a new Google account no longer hangs on Signing you in; Antigravity's limits wait until it is signed in; an ACP agent that isn't signed in offers a sign-in and a way back; Grok picks up its sign-in wherever it runs; OMP starts routed models on their own provider; a font change inside a CJK character no longer crashes; and the notch island's chime no longer stalls the app.

For the complete patch history, compare [`v0.6.2...v0.6.3`](https://github.com/nodelike/sikemux/compare/v0.6.2...v0.6.3).
