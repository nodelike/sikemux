# Sikemux v0.5.3

Sikemux 0.5.3 lets agents work together, adds tasks with a Run button, rebuilds the Git pane, and brings Linear and Vercel plugins, a skills library, shared chats and Linux packages.

## Agents work together

- Every agent gets a name, like Otter, shown in its tab, rows and header. Agents list each other, message each other, start helpers in a chat or a terminal, read what another said and wait on it. A worker opens beside its lead, sits under it in the rail, and its lead hears when its turn ends or fails. Each project keeps a log of the messages between its agents.
- Link the computers on your account in Settings, and an agent can start workers on another host, shown as Otter on studio.
- The rail, the notch island and your phone share one inbox: what needs you first, then what to review, finished work and what's running. Permissions and questions are answered right on the row.
- Agents ask their lead or you with `ask`, and Claude's, Codex's and Muse's own questions go the same way. Several questions open above the composer as tabs and go in one Send, and a secret question is answered in a masked field and reaches the agent as a private file.
- Agents sharing a checkout stay out of each other's way: their git refuses commands that sweep up others' work, they commit only their own lines, and they hear when they edit the same file. Every agent commit carries a footer the host signs, the Git pane shows which agent wrote each change, and `who_wrote` names who changed something or broke CI.
- Each project has a board of cards that agents add, claim and finish, linked to Jira. Auto starts an agent on each card in its own worktree, answers review comments, and merges only after an agent on another CLI has reviewed the diff.
- @ mentions reach agents in other projects, and a message sent while an agent works steers it by default.

## Tasks and the Run button

- Sikemux reads a project's build files and offers its tasks from a Run button: Node and Next.js, Python, Rust, Go, Maven and Gradle get run, test and checks, with test failures first and Rerun failed. Add your own in `.sikemux/tasks.json`, hand a failure to an agent, and an agent hears when a task it started fails.
- The run menu lists every running server and its owner, stops a run, and shows ports any program holds.

## The Git pane, rebuilt

- Changes, History and Branches swipe like the agent rail, the branch with its push and pull counts heads the panel, and the changes list reads like VS Code's Source Control.
- Pull requests, issues, runs and releases read like GitHub's, with a run's job graph, job logs step by step and CI beside every pull request. Agents see the pull requests a commit came in through.

## Chats and accounts

- Updating Sikemux carries running chat agents over to the new version, and a chat outlives its core.
- Share a chat read-only from the Mac. Anyone with the link watches it live in Sikemux, on their phone or at app.sikemux.com, encrypted end to end.
- Claude Code and Codex chats can run on a connected model provider, coding plans and local models included.
- Switching Claude or Codex accounts moves open chats with it, even mid-turn, and a chat stopped by a usage limit sends its prompt again once the limit resets. The usage limits lookup is fast again and no longer trips Anthropic's rate limit.
- Long Claude chats reopen past their compactions, a turn the network cut short goes again once it's back, and agents find and resume a past session from a vague memory.
- Pin chats to the top of the rail, and drafts follow you between the chat and the TUI.
- Install and sign in to agents from Sikemux: the welcome screen, Settings and the palette offer Install, Sign in and Reinstall, running each agent's own installer and sign-in.
- Muse Code joins the agents, and every agent can get a buddy, off by default.

## Skills

- Sikemux keeps a skills library that every agent loads beside its own, in a chat, a terminal or from a phone. Manage it in Settings → Skills, or let agents do it with a skills tool.

## Plugins

- Linear: issues in a pane, browser sign-in, and agent tools to read, comment on, move and file issues.
- Vercel: projects open on their production deployment with Instant Rollback and build logs, and agents can read and redeploy.
- Plugins run in the background core, so agents keep their tools while the window is closed. Bitbucket agents review, merge and run pipelines, Jira reads like Jira's own with filtered boards, AWS agents read ECS, logs, metrics and EC2, and Rundeck shows live deploys and history.

## Linux

- Every release ships a deb, an rpm and an AppImage, and one command installs Sikemux on a Mac or x86_64 Linux. `sh -s -- --host` puts just the core on a Linux server, and host commands are top-level words: `sikemux login`, `status`, `start`, `stop`, `restart`.

## Everything else

- The tab rail has pages for Agents, Terminals and Files, and the account sits at its foot.
- PDFs open in macOS's own PDF view, Java gets a language server, and a Markdown preview follows the file as an agent edits it.
- Dictation shows its words inside the message box as you speak.
- Themes preview as you move through the list.
- Panes reload and links reconnect when the network comes back or the computer wakes.
- rustls 0.23.46 fixes four advisories (RUSTSEC-2026-0346 to 0349), tool calls are tied to the agent whose processes make them, and only Sikemux's own app can replace the core.

Thanks to Ankit Patidar for the Linear plugin, pinned chats, drafts across GUI and TUI, agent sign-in and setup, the Jira board filters and the Escape fix; Krisna Satria for the Linux packages and fixes; and Sujal Rajput for the dictation preview and fixes.

For the complete patch history, compare [`v0.5.2...v0.5.3`](https://github.com/nodelike/sikemux/compare/v0.5.2...v0.5.3).
