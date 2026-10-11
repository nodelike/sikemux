# Sikemux v0.5.1

Sikemux 0.5.1 puts an iOS Simulator on the desk, drives Claude and Codex directly instead of through ACP adapters, lets agents show you pages they build, and adds plugins for databases and GitLab.

## Agents

- **Claude and Codex, driven directly.** Sikemux speaks each agent's own protocol instead of going through ACP adapters. Edit a sent message and ask again, and a loaded Claude chat shows what its subagents did.
- **Agents can show you a page.** An agent builds a page, checks it with `page_preview` and shows it with `page_show`, and the page sits in its reply. Sikemux keeps the pages agents build.
- **Agents say whether they are ready.** Every agent reports whether it is missing, broken, signed out or ready, and the welcome screen and agent palette tell you before you start.
- Claude chats show the model and effort their settings choose, and changing the model no longer leaves a chat stuck working.
- A running chat picks up the tools of a plugin you sign in to, without starting over.
- Agents without YOLO open in safe mode while YOLO is on.

## The chat

- Chats load faster and in full, open at the bottom and hold their place while you scroll, and tool rows draw an icon for what each call does.
- A finished answer folds its work above the reply, so the answer reads first, and each turn folds, shows its time and copies on its own. A reloaded chat keeps the real time of every turn.
- Right-click a recent Claude or Codex chat to delete it.
- Selected text in a chat is clearly visible, and the text cursor shows over it.
- Stopping a background task no longer leaves the chat stuck working.
- Dictating into a pane finishes cleanly and says whether words are still coming, and sending while you dictate waits for the spoken words.

## The iOS Simulator on the desk

- An agent's desk can hold a live iOS Simulator you use directly, with its bezel, side buttons and rotation. Agents drive it too through `sim_` tools: tap, type, swipe, read the log and take screenshots. Sikemux downloads the simulator helper the first time you need it, and Settings has a section for it.

## Plugins

- **Databases.** Connect to SQLite, PostgreSQL, MySQL and MariaDB, with passwords in the Keychain. Browse schemas and tables, write SQL in tabs where ⌘↵ runs the statement under the cursor, see results in a grid, copy them as CSV, search your query history and send results to an agent. Agents query through `db_` tools on a connection of their own, read-only unless you allow them to change data.
- **GitLab.** GitLab, on gitlab.com or your company's server, joins the Git pane: merge requests with their changes, commits, comments and approvals, issues, releases, your To-Do list, and pipelines you can follow, retry, cancel or start. Agents get GitLab tools too.
- **Jira boards.** A board view with its sprint and columns where cards move between columns, plus lists of what you reported, watch or viewed. Tick tasks in an issue's description, and open an issue in the browser or copy its link.
- **Bitbucket.** The Git pane shows whether a pull request can merge, Bitbucket's issues, its tags as releases and an inbox of pull requests waiting on you.
- **Rundeck** notifies you when a deploy you started succeeds or fails.
- The Activity page prices your agents' tokens and shows what they cost.

## Your phone and the notch

- Phones on your account are let in when you sign in, after Sikemux asks you once.
- Your computer hands a paired phone the pictures kept for a chat and how many background tasks each chat has running, so new phone features can arrive without a new phone app.
- The notch island marks agents that left shells or monitors running, and shows those shells even while it is closed. Picking an agent there brings Sikemux in front of the app you were in.
- Deleting your Sikemux account now waits 30 days, and signing in during that time offers to restore it.

## Everything else

- Spaces switch in a full-width track with line icons, and you edit one with a right-click.
- Agents show under the open project's row in the rail, and the Term row is gone.
- Files get Material icons, gitignored files are dimmed, and every folder above a changed file is coloured.
- Clicking an agent's notification opens that agent, and the DMG opens in a branded window.
- An SSH terminal reconnects when you focus it again after its connection dropped back to the local shell.
- Right-clicking a desk tab copies its path or link, and the desk's right edge shows again.

Thanks to Ankit Patidar for the simulator tab and its helper, agent sign-in status, the Database and GitLab plugins, Jira boards, Bitbucket's issues, releases and inbox, Rundeck run notices, dimmed gitignored files and desk tab menus, and to Sujal Rajput for the agents' simulator tools and its Settings section.

For the complete patch history, compare [`v0.5.0...v0.5.1`](https://github.com/nodelike/sikemux/compare/v0.5.0...v0.5.1).
