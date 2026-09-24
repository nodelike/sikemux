# Sikemux v0.4.1

Sikemux 0.4.1 turns the panes beside your agents into plugins, adds SigNoz as the first new one, and gives an agent's browser the reach of a real person at the keyboard.

## Plugins

- AWS, Rundeck, Bruno and the new SigNoz pane are plugins now, gathered in one Plugins group in the rail. Each can be switched off, and each loads its own styles only when its pane opens rather than at every launch.
- A plugin can offer tools to your agents, bring its own shortcuts, and open its documents as tabs in the workspace strip.

## SigNoz

- Sign in with email and password or an API key, and read a live error feed, logs, traces with a waterfall, and dashboards. Services open as pages of their own, with their charts, endpoints and grouped errors.
- ⌘P jumps to a service, an action, or a pasted trace id.
- Agents can read the same services, logs, traces and dashboards.

## The agent's browser

- An agent's clicks and keys arrive as real input. It can run JavaScript in its page, answer a page's alert, confirm and prompt, reach controls inside frames, attach files, and read what the page logged.
- It can draw on a page and capture all of it, and record its tab to a video. The Browser toggle lights up while its browser is open.

## Agents and the chat

- A ring in the composer shows how full the context window is, and a resumed session shows it before its next turn.
- A pasted image attaches to the message, the composer grows with its draft, and the agent is picked from the top of the model menu.
- Agents waiting for input are gathered across projects in the rail, and open agents sort by what needs attention.

## The workspace

- Drag a tab to a new place in the strip, resize text with ⌘=, ⌘− and ⌘0 in the terminal, chat and editor, and drag either rail's edge to resize it.
- Themes: bring in Ghostty's 463 themes behind a searchable picker, or draft one from your desktop wallpaper. Settings gained search and keyboard navigation.
- A new Activity page records the agent work done inside Sikemux, and What's New shows the running build's notes and who made them.

Thanks to Ankit Patidar for #15, #16, #23, #24, #25, #26, #30 and #31.

For the complete patch history, compare [`v0.4.0...v0.4.1`](https://github.com/nodelike/sikemux/compare/v0.4.0...v0.4.1).
