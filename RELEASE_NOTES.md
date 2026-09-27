# Sikemux v0.4.2

Sikemux 0.4.2 lets you talk to your agents, brings four more agents into the built-in chat, lets the workspace split, gives the whole app motion, and closes a long list of ways a page, a repo or a project file could reach further than it should.

## Agents and the chat

- Hold right Option to dictate into the focused agent, or use the microphone in the composer. Speech is transcribed on your Mac, on the Neural Engine, and the words appear as you speak them.
- OpenCode, OMP, Grok and Hermes open in the built-in chat, each over its own ACP.
- Tool calls show what they printed and what is still running. Code blocks have a copy button and a language icon, and markdown is read by a Rust parser.
- The up and down arrows bring back messages you already sent. A new agent tab is named after its first prompt.
- Sikemux notifies you when an agent needs you while the app is in the background.

## The workspace

- Workspace tabs live in the top bar. Drag a tab onto the screen to split it above, below or beside the current one, up to three across. Git, search and the editor split too, and a file can split beside itself.
- Files opened from the tree open in a preview tab, and the tree is a panel of its own.
- An agent's side pane holds its pages, files and task terminals.
- Cmd+B toggles focus mode. Pinch the trackpad to resize text in the chat, the editor and the terminal.
- Diffs are drawn by Sikemux itself, from libgit2.

## Look and motion

- Tabs, rails, menus, palettes and toasts open and close with motion. New messages rise in, tool calls unfold in place, and working agents twinkle instead of spinning.
- First run is a single welcome screen.
- AWS and Rundeck are redesigned around a list and a side panel.
- The terminal colours shell output and spaces its rows the way Ghostty does.
- Selection shows as a ring, never a stripe down the left edge.

## The agent's browser

- A tab can be held at a fixed viewport, the network log lists page loads and framework requests, and actions can find and click by label.
- Tabs load only web addresses, finished downloads are quarantined, and pages are refused the camera and microphone. A page can no longer open tabs or dialogs in a loop.

## Safer by default

- A project's language servers, and the commands a `sikemux.json` asks to run, wait for your approval.
- A repo's own git config can no longer run commands when Sikemux reads files, and ref or remote names are never read as options.
- Speech models download pinned and hash-checked, and the app window cannot be navigated away from the app.

For the complete patch history, compare [`v0.4.1...v0.4.2`](https://github.com/nodelike/sikemux/compare/v0.4.1...v0.4.2).
