# ADR 0012: Agents talk to agents through the core

- Status: Accepted
- Decision date: 2026-10-14

## Context

People run several agents on one project, often from different CLIs, and want one to hand
work to another: Claude plans while Codex builds, a lead splits a ticket among workers, and
everyone answers questions without the person relaying text between panes. Claude Code
ships its own `ListAgents` and `SendMessage`, but only between Claude sessions, and a
session that skips permission prompts holds every incoming message for approval; a chat
the core runs headless cannot show that prompt and drops the message after five minutes.
Claude's channels, which push text into a session, ignore headless sessions. Neither
reaches Codex or the ACP agents.

The core already drives every chat agent itself (ADR 0006): it starts them, sends prompts,
steers a running turn without stopping it (Claude takes a message at its next tool call,
Codex through `turn/steer`), and records every event. It also serves the `sikemux-tools`
MCP server every agent is launched with.

## Decision

- **Agent-to-agent is core tools, not a protocol.** `agent_list`, `agent_spawn`,
  `agent_send`, `agent_read` and `agent_wait` are `sikemux-tools` methods answered in the
  core's harness, so they work for every CLI and with the window closed. The A2A protocol
  is left for a later adapter at the edge, for agents outside Sikemux.
- **Every agent has a name.** The core gives each agent a short noun from a built-in list
  (Otter, Maple, Flint) when it starts, unique among the project's open agents. An agent
  holds its name only while it is open: closing it frees the name, and a chat opened again
  gets a new one. Leads do not choose names. The person can rename one. Tools, `@`
  mentions in the composer, tabs and the rail all use the name.
- **The core knows who is calling from the processes it started.** The core starts every
  agent and records its process and start time. Every tool call is the call of the agent
  whose process tree holds the connection, whatever agent id it states. A call from outside
  every agent's tree, including one re-parented to launchd or init, is no agent's: the
  tools that act as an agent (agents, board, git, review, browser and simulator) refuse it,
  and the rest, such as tasks, `workspace_inspect` and `ui_open`, answer it with no agent,
  as they answer the person's own `sikemux tool` in a terminal. Nothing
  secret travels in an agent's environment or tool config, where any process of the same
  user could read it. An update runs the new core in the same process, so the agents stay
  its children and it records them again as it takes them up.
- **Only Sikemux's own window and notch act as the person.** The core checks the code
  signature of the process on the other end of its socket. Answering questions and
  permissions, adding board cards as the person and board settings need a
  verified window or notch; an agent's shell may read but not act as the person. A build
  not signed by Sikemux's team trusts the app and notch in its own bundle by path, as a dev
  build does; a team-signed build never falls back to a path.
- **Messages are text, labelled as coming from an agent.** A message can never answer a
  permission prompt or change configuration. Any agent may message any other, across
  projects, except Auto's reviewer: it is the one agent others cannot message, read or
  wait on, and it cannot reach them.
- **Delivery steers first.** A busy agent that can steer reads the message at its next
  tool call; an idle one gets it as a new turn. Only an agent that can do neither, such as
  a terminal agent mid-turn, has it queued in the core, up to ten, and delivered when its
  turn ends. A worker's report to its lead is never turned away; only its newest waits.
  Two agents that keep messaging each other with no other work between them are held after
  a few rounds, and the person's inbox names them. Each project keeps a bounded log of who
  messaged whom and what became of it, never the text.
- **A spawned agent is a full agent.** It opens in its own tab, takes its lead's account
  and permission mode (the lead may pick the CLI and model, never the mode), and is
  recorded with the agent that spawned it. When its turn ends, its final answer goes to its
  lead. Spawning is capped at two levels deep, six children per agent and eight spawned
  agents per project.
- **Questions go to the lead first.** An `ask` tool, which Claude's own `AskUserQuestion`
  routes into, sends a worker's question to its lead; the lead answers or passes it to the
  person. Codex's and Muse's own questions go the same way. A lead's message closes only the
  question it names as answering. A secret question goes straight to the person, and its
  answer reaches the agent as a private file, never as text in a transcript or log. A worker
  that is not blocked goes ahead on a stated assumption, and a card whose team has an open
  question or an unseen assumption does not merge.
- **Auto's merge needs a blind review beside the questions gate.** Once a card's checks pass,
  Auto starts a reviewer of its own on another CLI than the builder's, with no lead and no
  session to resume. It reads only the card, its acceptance criteria, which only the person
  writes, and the diff from the run's start to the pushed commit, in a read-only checkout of
  that commit. Its approval is kept against that commit, so any later push needs a new one.
  Findings go back to the builder as review comments do, and each round gets a fresh
  reviewer. With no signed-in agent on another CLI, or no verdict, the card waits for the
  person.
- **Work is tracked on one board per project.** Cards have a parent and dependencies and no
  types. Claiming is a single step in the core. Only the person moves a Jira or Linear
  ticket onto the board, and the board's automatic pickup starts agents only for cards the
  person added.
- **Agents sharing a checkout are told when they collide.** The core keeps, for each file an
  agent read or changed this session, the copy that agent last saw and the file's last few
  versions, in memory within a fixed budget. When an edit lands on lines another open agent has
  uncommitted, both and their leads get one note per burst, from Sikemux, and both are marked in
  the rail and on phones. A write made from an older copy is merged on disk with what changed
  since: where both changed the same lines the file keeps the other side, lines no agent wrote
  counting as the person's, and the writer is told which lines to redo. The first agent to edit a
  lockfile, migration or generated file holds it for a few minutes; another's edit still goes in,
  and both are told. After each `git_commit` the board's check command runs in the checkout and
  its failure goes to the agents the commit's trailers name. Two agents that keep colliding, and
  a check that keeps failing, reach the person's inbox once.

## Consequences

- Every way an agent's tools reach the core must run inside the agent's own process tree.
  Claude, Codex, OpenCode, OMP, Grok, Hermes and Pi start their MCP servers as their own
  descendants; a CLI that moved them into a shared daemon would lose its agent tools.
- The core keeps per-agent names, parent links, queued messages and open questions on disk,
  so they survive a window reload, a core upgrade, a crash and a restart.
- Messages, questions and the board move on events and deadlines of their own, not a
  polling tick.
- Collisions are seen from what the drivers report: reads and edits by tool, and a watch on
  the folders of the files agents share for changes no tool reported. Claude refuses a write
  from a stale copy itself, so the core mends the others' writes right after they land, having
  kept the file as it stood when the tool started. A change no edit tool reported is mended only
  when one chat agent was running a command at the time; otherwise its owners are only told,
  since it may be the person's own.
- The rail lists workers flat, right after their lead, joined by the lead's tree line.
- Phones reach the same tools as `Call` methods (ADR 0010), never new wire requests.
- A lead that is compacted or closed does not lose the plan: it lives on the board.
- A reviewer and its checkout go when its review ends, when its card leaves Auto, or, after a
  restart that lost it, when Auto starts the review again on the same commit.

## Notes

- 2026-10-15: `agent_spawn` takes `host`, a host linked with this one
  ([ADR 0013](./0013-hosts-link-to-run-agents-for-each-other.md)), and starts the worker there
  on that host's own sign-in, in a checkout of its own of the lead's code. The worker is
  addressed by its host and its agent id and shown as "Otter on studio". Messages, reports,
  questions, `agent_read` and `agent_wait` work across the link under the same rules, the caps
  count the team on every host, and the worker's commits come back to a branch in the lead's
  checkout before each report.

## Alternatives considered

- **A per-agent secret in the environment.** Shipped first and replaced: on macOS any
  process of the same user reads another's environment (`ps -E`), so an agent could take
  another's secret and act as it.
- **Claude's own cross-session messaging.** Rejected: Claude only, and it drops messages
  to the headless, prompt-skipping sessions Sikemux runs.
- **Claude channels for delivery.** Rejected: research preview, needs a development flag
  with a warning screen, and is ignored in headless sessions.
- **The A2A protocol inside Sikemux.** Rejected for now: a network endpoint and identity
  model for agents that are all local processes the core already drives.
- **Holding messages from other projects for approval.** Rejected: every agent is the
  person's own and can already reach the other project's files.
- **Letting the builder pick or brief its reviewer.** Rejected: a reviewer that shares the
  builder's CLI, context or account of what it meant to do shares its blind spots.
- **Nesting workers under their lead in the rail.** Rejected: rows already hang their own
  project, branch and pull request tree, and a tree inside a tree truncates titles.
