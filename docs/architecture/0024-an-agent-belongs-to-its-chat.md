# ADR 0024: An agent's id and name belong to its chat

- Status: Accepted
- Decision date: 2026-11-12
- Replaces: the naming line of [ADR 0012](./0012-agents-talk-to-agents.md)

## Context

ADR 0012 gave an agent its name, and its id, for one opening of a chat. Closing the tab freed
the name at once, and opening the chat again made a new agent with a new id, name, face and
colour, no lead, and none of its board cards.

Across 1,633 past transcripts and the live name book that cost:

- 52 `No open agent is called X` failures in 39 sessions, about half of them messages dropped.
- Leads keeping rename tables by hand: more than 75 "Dororo (was Satsuki)" lines, one chat
  wearing six names in three days, briefs going stale mid-run.
- Board cards stuck because their owner's id changed on reopen (#52, #14).
- 89 unrelated chats given a name another chat had worn, the fastest 31 minutes later, and one
  `agent_close` by old names that nearly closed the wrong agent.

## Decision

- **A chat is one agent for life.** Taking a saved session up again, from Recent, Undo, a
  link, `@`, `agent_spawn` with `resume`, `agent_send` to a closed worker, or a click on a
  closed agent's name, brings back the same agent id, name, lead, account and look. Only when
  an open agent of the project holds the name by then does it come back under a fresh one. A
  new chat, and a fork, is a new agent.
- **Names are reused oldest first, and only when the list runs out.** A closed chat keeps its
  name; a new agent gets a word no open or closed chat of the project holds while one is left,
  then the name of the project's longest-closed chat. A name the person chose is never reused.
  A record that falls off the cap of closed records frees its name and id.
- **A bare name reaches only the caller's project and team.** Elsewhere the agent id is needed.
- **Text that names an agent says which.** Notes to agents give the id beside the name, and a
  click on a closed agent's name in the app opens its chat again.

## Consequences

- Names and ids stay true for as long as anyone holds them: in briefs, board cards, goals and
  the person's memory of who did what.
- The name book keeps closed chats' names, so a project with many closed chats goes through
  the word list before reusing one.
- A name can still change, when the person renames an agent or another open agent holds it on
  return; the id never does.
