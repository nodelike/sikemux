# Sikemux v0.6.1

Seventeen days and about 3,100 changes after 0.6.0. Every project gets a chief, what an agent needs from you arrives as a call, chats are redrawn, and the iPhone app is native.

## Before you update

- **Your phone needs the phone app 0.2.0 or later.** On iPhone it is on the App Store. On Android it comes from the Google Play testing track. The older phone app, 0.1.0, can't connect.
- **Hosts link only with hosts on 0.6.1.** Update every computer you link, or they can't reach each other until they do.
- **Linux: the .rpm is gone.** Use the AppImage instead. The app needs Ubuntu 24.04, Debian 13 or a system as new; a host's core still runs on older ones, back to Ubuntu 22.04 and RHEL 9.
- **Two things change once.** What you asked your agents becomes tasks on the project's board, with new numbers. Agents get anime names; Settings › Agents › Agent names puts the old ones back.

## What's new

- **A chief for every project.** The chief heads the project's agents, tells you what happened while you were away, and lists the decisions agents made, each with Overrule and Undo.
- **Calls.** What an agent needs from you is one card with your choices. The bell in the top bar gathers calls from every project and host, the desk has a Calls page, and they reach the notch and your phone.
- **Chats, redrawn.** Your messages are bubbles, a run of tool calls folds into one line that says what it did, code blocks are cards with Copy, and Mermaid diagrams and LaTeX draw in replies. A line under the chat says what the agent is doing or waiting on.
- **Find and branch chats.** Search every chat by what was said in it, fork a chat, hand it to another agent, or open a side chat with /side.
- **Agents that work as a team.** Agents start and close their own helpers and land their commits through a queue that runs your checks. Heavy commands such as builds wait in one line, so the computer stays usable, and each checkout gets its own ports.
- **Accounts and limits.** With several accounts for one agent, a new chat starts on the one with the most room, and limits show for each account.
- **More agents.** Pi and OpenCode run as full chat agents, OpenCode's free models need no sign-in, and more model providers are built in.
- **Hosts and devices.** The Devices page shows each host's CPU, memory and disk, and says why a host is out of reach. A project is one project across your hosts, and a chat can move between them.
- **iPhone, native.** The phone app is rewritten in Swift, with a Live Activity for your running agents.
- **Themes.** A theme is a file that applies as you save it: colours, fonts, density and a backdrop. Settings › Appearance adds Contrast and Motion.
- **GitHub.** Agents can push as their own GitHub App, so they never hold your token, and CI status arrives the moment it changes.
- **Linux.** arm64 builds, an apt repository for the .deb, and an Arch package.
- **Lighter and steadier.** An idle chat sleeps after two minutes and frees its memory, the app uses much less GPU memory, and a Mac update that crashes at start puts the previous version back.
- **Homebrew.** `brew install --cask nodelike/tap/sikemux`.

Also fixed: a wide character in a one-column terminal could end every agent on the host, a sent message could go out a second time, closing a terminal's tab left its shell running, and agents' browser tabs lost their sign-ins after a week.

Thanks to @Sujal85526 for five fixes to chats and updates.

For the complete patch history, compare [`v0.6.0...v0.6.1`](https://github.com/nodelike/sikemux/compare/v0.6.0...v0.6.1).
