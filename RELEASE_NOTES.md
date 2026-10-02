# Sikemux v0.4.3

Sikemux 0.4.3 keeps your agents running when the window closes, brings your code host into the Git pane, and lets you hand an agent anything you are looking at.

## Agents keep running

- Terminals and chat agents run in a background core, so quitting Sikemux leaves them running and reopening it puts every pane back where it was. A chat agent comes back mid-turn, with its permission request still waiting. Quit and Stop Everything ends them all, and `sikemux core stop` does the same from the command line.
- Updates no longer stop anything. A newer Sikemux takes over the running core in place, panes keep their screens, and an update waits for chat agents to finish their turns.
- A terminal agent that crashes comes back on its conversation in the same pane, a chat whose agent died can be resumed, and agents' tools keep answering with the window closed.

## Your code host in the Git pane

- GitHub and Bitbucket Cloud live beside your local changes. Pull requests read like the host's own page, with their conversation, reviews, checks and who merged them. Check one out from its page, open a failing check's job, or jump from a CI annotation to its file and line.
- GitHub Actions runs with their jobs, logs, artifacts and deployment approvals, Bitbucket Pipelines, releases and an inbox, which your agents can read too.
- More than one account per host, switched per project.
- A new Git pane: Changes, History and Branches over one review, staging from a file's row, one commit message box, and history that shows who wrote each commit.

## Working with agents

- Hand an agent a terminal selection, a problem from the editor, SigNoz log lines or a pull request's diff lines, or start a new chat on an issue. In the composer, `@` attaches a file or folder and `#` hands over an issue or pull request.
- Find text in a conversation with ⌘F, and Escape stops the running turn. Each prompt shows when it was sent and how long its answer took, and queued messages go out together.
- Start a chat in its own git worktree; Sikemux shows its pull request and cleans it up once merged.
- Rename chats, browse saved chats across providers and projects in the All agents tab, reopen one from a `sikemux://` link, and switch Claude or Codex accounts from the limits footer.
- A chip in the top bar counts the ports your project's terminals, tasks and agents listen on.

## The workspace

- ⌘N starts an agent, ⌘T a terminal, ⌘J the desk. ⌘W no longer closes the window, and closing a project or a working agent asks first.
- PDFs, audio, video, fonts and binaries open in a preview, and Office and iWork documents through Quick Look.
- The desk's browser suggests the sites you visit most as you type an address.
- Aura Noir is the default theme.

## The agent's browser

- Its keys and clicks stay in the tab and leave your cursor and typing alone. It can open local files, wait on a condition, reload, and act several times in one call, and a tab notices when its page hangs or crashes.
- An agent is offered only the plugin tools that work in its repository.

## Remote access, first steps

A Devices page in Settings pairs a device with a code you approve, and a paired device can see and start chats in your projects, drawn in your Mac's theme. There is no phone app yet; this is the groundwork for one.

Thanks to Sujal Rajput for the GitHub plugin and renaming chats, to Ankit Patidar for find in conversation, Escape to stop, turn timings and keeping Git's state fresh, and to Krisna Satria for the start of a Linux build.

For the complete patch history, compare [`v0.4.2...v0.4.3`](https://github.com/nodelike/sikemux/compare/v0.4.2...v0.4.3).
