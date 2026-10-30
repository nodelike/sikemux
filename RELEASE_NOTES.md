# Sikemux v0.6.0-nightly.11

The eleventh nightly of the new Sikemux app. Nightlies are signed and delivered exactly like stable releases, but they carry unreleased work and can break. Switch back to stable in Settings → About whenever you want; you keep the build you are on until a stable release passes it.

Your phone needs the nightly phone app, 0.2.0-nightly.3 or later, from TestFlight on iPhone or the Google Play testing track on Android, and nightly hosts link only with other nightly hosts.

## New since nightly.10

- **A Stash that reads your links.** The Stash tray lists items as rows, newest first, and the server reads the first pages a stashed prompt or text links to, with each site's icon; pull requests and issues show in their state's colour.
- **DBML diagrams remember.** Where you moved a diagram's tables, and its pan and zoom, survive restarts and updates, and references over several columns meet their tables cleanly.
- **Agents manage their teams.** An agent can close the agents it started, keep 16 teammates open at once, up from 6, and a project can hold 64 agent-started agents.
- **Tabs say what's going on.** An agent's tab shows its shells, subagents and next timer, as its rail row does.
- **Jira cards drag.** Drag a card to another column of its lane on the Jira board, as on the agents' board. Thanks to @ankit-ml12.
- **Faster, typed talk with the core.** The window reads the core's answers as typed values, so chats and lists update with less work.
- Links to pull requests and issues read as GitHub writes them, and other hosts' folders show under ~ from that host's own home.
- Tab and Shift-Tab reach a terminal's program again, and a close button's click no longer also opens what it closes.
- A task an agent runs again replaces its earlier terminal on the desk, and showing local files no longer runs out after 16.
- The top bar's CI chip settles when a run finishes, and an agent's header shows its worktree's pull request again.
- The Sikemux mark sits centred in every icon.

For the complete patch history, compare [`v0.6.0-nightly.10...v0.6.0-nightly.11`](https://github.com/nodelike/sikemux/compare/v0.6.0-nightly.10...v0.6.0-nightly.11).
