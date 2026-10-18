# Sikemux v0.5.3-nightly.7

The seventh nightly on the 0.5.3 line. Nightlies are signed and delivered exactly like stable releases, but they carry unreleased work and can break. Switch back to stable in Settings → About whenever you want; you keep the build you are on until a stable release passes it.

## New since nightly.6

- **Tasks and a Run button.** Sikemux reads a project's build files and offers its tasks from a Run button: Node and Next.js, Python, Rust, Go, Maven and Gradle get run, test and checks, with test results failures first and Rerun failed. Add your own tasks in `.sikemux/tasks.json`, hand a failure to an agent, and Sikemux checks a task's build tool, Java and port before it runs. An agent hears when a task it started stops with an error.
- **The run menu and ports.** The run menu lists every running server and its owner, stops a run, and shows ports any program holds. The port check no longer raises the firewall prompt.
- **The tab rail has pages.** Swipe between Agents, Terminals and Files, like Zen's spaces. The Files page shows the project tree under the open files, file and terminal rows are tabs, and the account moves to the rail's foot with a sign-in card and settings.
- **Vercel.** A new Vercel plugin signs in through the browser and lays out like Vercel's dashboard: projects open on their production deployment with Instant Rollback, each deployment says how long its build ran, and build logs are a click away. Agents can read and redeploy.
- **The Git pane, rebuilt.** Changes, History and Branches sit at the side panel's foot and swipe like the agent rail, the branch with its push and pull counts heads the panel, and the host's inbox is its fourth page. Pull requests, issues, runs and releases read like GitHub's: conversations with pushed commits, a run's job graph laid out from its workflow file, job logs step by step, and CI beside every pull request. A folder holding several repositories can have a default one.
- **@ mentions reach agents in other projects too,** listed under their project.
- **Pinned chats.** Pin a chat from its menu, or from the pin beside an open agent's close, and it stays at the top of the rail, open or closed, with running chats above it.
- **Drafts follow GUI and TUI.** What you typed in the chat waits at the TUI's prompt after a switch, and what you typed in the TUI comes back to the chat.
- **Git.** The changes list reads like VS Code's Source Control, History has its own tab, pull requests and issues open as screens of their own, and Actions and Releases join them in the project tree. A changed file shows the faces of the agents that wrote it, and staging shows at once.
- **Agents.** Review an agent's work from its branch, new worktrees get their `.env` files, and a started Claude or Codex chat moves to another project with its history. A message sent while the agent works steers it by default, with queuing as the setting's other choice. A chat outlives its core, and an agent that dies under its chat is started again.
- **Sign in and Reinstall.** A signed-out agent offers Sign in in Settings and the palette, a broken one offers Reinstall, and Linear signs in through the browser.
- **Linux hosts.** `sh -s -- --host` puts just the core on a Linux server, starts it at boot and signs it in, and every host command is a top-level word: `sikemux login`, `status`, `start`, `stop`, `restart`.
- **Java** gets a language server through jdtls, and opening an editor loads only the language it needs.
- Tags in the composer are chips with their own prefix, and the menu docks onto the composer.
- Opening a browser tab no longer pulls the keyboard from the app you're typing in, and an agent's `ui_open` no longer raises the window over another app.

Thanks to Ankit Patidar for pinned chats, drafts that follow the switch between GUI and TUI, and Sign in and Reinstall.

For the complete patch history, compare [`v0.5.3-nightly.6...v0.5.3-nightly.7`](https://github.com/nodelike/sikemux/compare/v0.5.3-nightly.6...v0.5.3-nightly.7).
