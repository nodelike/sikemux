# Sikemux v0.5.2

Sikemux 0.5.2 adds a Slack plugin, makes shells start faster, and makes Claude chats follow Claude Code's own records of turns, tasks and subagents.

## Slack

- A new Slack plugin signs in with a Slack app's token, for one or more workspaces, and keeps each token in the Keychain. A Slack pane shows channels, messages and threads; reply in them, open links, and send a thread to an agent. Agents get `slack_thread`, `slack_search`, `slack_post` and `slack_user`, and Slack links in a chat are marked.

## Faster shells

- Sikemux's background core now runs at normal priority, so shells and prompts no longer start at background priority, where rc files took about twice as long to load. Sign out of macOS and back in once for it to take effect.
- A new terminal tab opens at its prompt straight away, from a shell kept ready.

## Chats and subagents

- A new chat is usable while its agent starts, and a message you send during a Claude turn joins it after the next tool call.
- Claude chats follow Claude Code's own reports of turns, tasks and subagents. A reloaded chat reads its subagents from Claude Code's own files, including ones you stopped, which show as cancelled. Editing a message rewinds in place and keeps its background work. A reloaded Codex chat shows its subagents as they ran.
- A subagent's own subagents stay inside its card, a resumed one reopens its card and stops spinning when it finishes, and background subagents stay visible after you stop a turn. A background shell says which subagent started it, and a turn says which background work woke the agent.
- An agent process that ends takes its subagents and tasks with it, an agent with only a shell left running is no longer listed as working, and background shells are counted beside the >_ mark.
- Copy a table in a chat as markdown. A turn that only thought folds under "Thought for", and streaming text stays whole when a task notice lands mid-reply.
- Closing anything that would stop a busy agent asks first.

## Everything else

- Jira lists filter by status, with a count for each, and agents can file an epic's children, edit issues and link them.
- Markdown files open in preview, and in source when opened at a line. Enter in the find box goes to the next match.
- Language servers start for every language the app can serve, and the PDF preview loads again.
- Git no longer shows conflicted files in green, and the address bar opens files and local network hosts the way agent tools do.
- Claude transcripts are found for projects whose path has dots, underscores or spaces.
- Moving an agent to another project no longer freezes tab switching, and All projects orders done and idle agents by when they last worked.
- Keyboard shortcuts show each platform's own modifier names, the battery shows charging when it is low, and the SigNoz live tail pauses while the app is hidden.
- Agents prefer a plugin's tools over another server's for the same service.

Thanks to Ankit Patidar for the Slack plugin, the Jira status filter and the find box fix.

For the complete patch history, compare [`v0.5.1...v0.5.2`](https://github.com/nodelike/sikemux/compare/v0.5.1...v0.5.2).
