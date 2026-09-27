# Sikemux v0.4.2-nightly.3

The third nightly on the 0.4.2 line. Nightlies are signed and delivered exactly like stable releases, but they carry unreleased work and can break. Switch back to stable in Settings → About whenever you want; you keep the build you are on until a stable release passes it.

## New since nightly.2

- **Voice.** Hold right Option to dictate into the focused agent, or use the microphone in the chat composer. Speech is transcribed on your Mac, on the Neural Engine.
- **More agents in the chat.** OpenCode, OMP, Grok and Hermes open in the built-in chat, over their own ACP.
- **Motion.** Tabs, rails, menus, palettes and toasts open and close with motion. New messages rise in, tool calls unfold in place, send and stop turn into each other, and working agents twinkle instead of spinning.
- **A new first run.** One welcome screen instead of a four-step tour.
- **AWS and Rundeck redesigned** around a list and a side panel. AWS also shows queue message counts, and a task's zone and address.
- **The chat.** Tool calls show what they printed and what is still running. Code blocks have a copy button and a language icon. A new agent tab is named after its first prompt, and Claude sessions are titled by what you typed.
- **The desk.** An agent's side pane holds its pages, files and task terminals.
- **Languages.** Grammars that don't ship with the app are downloaded once, when first needed, and files without an extension take their language's icon.
- **The terminal** colours shell output and spaces its rows the way Ghostty does.
- Selection shows as a ring instead of a stripe down the left edge.
- Tauri 2.12, and a round of dependency upgrades.

For the complete patch history, compare [`v0.4.2-nightly.2...v0.4.2-nightly.3`](https://github.com/nodelike/sikemux/compare/v0.4.2-nightly.2...v0.4.2-nightly.3).
