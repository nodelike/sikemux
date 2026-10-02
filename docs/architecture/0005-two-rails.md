# ADR 0005: Sessions and agents keep separate rails

- Status: Accepted
- Decision date: 2026-09-21

## Context

The window has two side columns: the side rail (`src/rail/SideRail.tsx`) lists
sessions, and the agent rail (`src/rail/AgentRail.tsx`) lists agent chats.
Together they take about 546 px, which prompted attempts to merge them.

About fifteen single-rail layouts were mocked up on 2026-09-20 and all were
rejected. One was then built anyway and shipped in v0.4.0-nightly.9
(`ebb861d1`, one rail with a page per project). It was reverted the next day
(`e3b5b029`) because the layout read worse in use.

## Decision

Keep two rails. Do not merge them, and do not turn agents into items in the
project tree beside Files and Git.

The rails hold three things: which session (six kinds), which window, and which
chat (dozens per project). The session groups (Projects, SSH, Cloud, CI/CD, API,
Command) are named, and each header carries its own shortcut and actions: `+`
means something different in every group, and the SSH pencil opens
`~/.ssh/config`. Keeping the named groups, showing the full agent list, and
using one rail can be had two at a time, never all three. One column holding
both has to fence them apart, and that fencing is what read badly every time.
An icon-only dock drops the group headers and is the worst of the options.

## Consequences

- To win back width, use the existing `toggleAgentRail` and the `RailPeek`
  hover reveal, or narrow the rails. Do not restructure them.
- Treat this as settled. It held through a full implementation and a release.
