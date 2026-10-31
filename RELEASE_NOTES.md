# Sikemux v0.6.0

Sikemux 0.6 is a new app. It is drawn natively on the GPU instead of in a web page, so it starts faster, stays light with many agents at work and long chats, and keeps everything you had: your projects, agents, chats, accounts, settings and plugins carry over.

## Before you update

- **Your phone needs the new phone app, 0.2.0 or later.** Install it from TestFlight on iPhone or the Google Play testing track on Android. The older phone app, 0.1.0, can't connect to hosts on 0.6.
- **Hosts link only with hosts on 0.6.** Update every computer you link, or they can't reach each other until they do.
- **There is no going back to 0.5.** This release moves saved chats to a new format that 0.5 can't read.

## What's new

- **Native and fast.** The whole app is drawn on the GPU, with typed, lighter talk between the window and the core, and tabs out of sight cost nothing.
- **Chats that keep their place.** Agents start only when you write, chats open with their history, pick up exactly where they left off after a reconnect, and resume on the model they last ran.
- **Your own model providers.** Add any provider with its base URL and key, kept in the Keychain, and run OMP and OpenCode chats on its models.
- **Stash.** Share links, text, photos and files to Sikemux from your phone's share menu, and add them to any chat from the composer's Stash button.
- **Triggers.** Agents can prompt their own chats later, on a schedule, from the board, or from a webhook that reaches your computer wherever it is.
- **Teams of agents.** An agent can start, message and close the agents it works with, and its tab shows what each one has going.
- **Pull requests and CI in the app.** Pull requests read like GitHub's, links to pull requests, issues, runs and Jira issues open on the agent's desk, and the top bar's CI chip stands for your branch's newest commit.
- **Databases and diagrams.** Connect PostgreSQL, MySQL or SQLite, browse tables and run SQL, and see `.dbml` files and replies as diagrams you can move around.
- **Plugin panels.** Jira, Linear, Slack, databases and more open as panes, with Jira cards you can drag between columns.
- **On Linux too.** The app ships for x86_64 as a .deb, an .rpm and an AppImage, and a Linux host updates itself.
- **Crash reports.** A crash or a window that stops answering leaves a report that `sikemux doctor` shows.
- **Settings regrouped** into six groups, menus that work with the keyboard, find in markdown, HTML previews, and videos that play in Quick Look.

Thanks to @ankit-ml12 for pasted pictures, find in markdown, parked browser tabs and Jira drag and drop.

For the complete patch history, compare [`v0.5.3...v0.6.0`](https://github.com/nodelike/sikemux/compare/v0.5.3...v0.6.0).
