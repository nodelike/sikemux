# Sikemux v0.5.2-nightly.2

The second nightly on the 0.5.2 line. Nightlies are signed and delivered exactly like stable releases, but they carry unreleased work and can break. Switch back to stable in Settings → About whenever you want; you keep the build you are on until a stable release passes it.

## New since nightly.1

- **Faster shells.** Sikemux's background core now runs at normal priority, so shells and prompts no longer start at background priority, where rc files took about twice as long to load. Sign out of macOS and back in once for it to take effect. A new terminal tab opens at its prompt straight away, from a shell kept ready.
- **Chats start sooner.** A new chat is usable while its agent starts, and a message you send during a Claude turn joins it after the next tool call.
- **Claude chats follow Claude Code's own records.** Turns, tasks and subagents follow what Claude Code reports, and a reloaded chat reads its subagents from Claude Code's own files, including ones you stopped, which now show as cancelled. Editing a message rewinds in place and keeps its background work. A reloaded Codex chat shows its subagents as they ran.
- An agent process that ends takes its subagents and tasks with it, and an agent with only a shell left running is no longer listed as working. Background shells are counted beside the >_ mark.
- Closing anything that would stop a busy agent asks first.
- Claude transcripts are found for projects whose path has dots, underscores or spaces.
- Git no longer shows conflicted files in green, and the address bar opens files and local network hosts the way agent tools do.
- Language servers start for every language the app can serve, and the PDF preview loads again.
- Keyboard shortcuts show each platform's own modifier names, the battery shows charging when it is low, and the SigNoz live tail pauses while the app is hidden.
- The notch takes each agent's look, its asks and model choices from the core.

For the complete patch history, compare [`v0.5.2-nightly.1...v0.5.2-nightly.2`](https://github.com/nodelike/sikemux/compare/v0.5.2-nightly.1...v0.5.2-nightly.2).
