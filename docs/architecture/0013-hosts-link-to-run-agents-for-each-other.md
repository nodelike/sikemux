# ADR 0013: Hosts on one account link to run agents for each other

- Status: Accepted
- Decision date: 2026-10-15
- Amended: 2026-10-19, so that a linked host's agents use none of this host's sign-ins until the
  person allows that host
- Superseded in part: 2026-10-21, by [ADR 0018](./0018-the-account-is-the-trust.md): hosts on one
  account link to each other without the owner allowing each link, and an agent keeps this host's
  sign-ins whichever host started or messaged it

## Context

People run Sikemux on more than one computer: a laptop, a desktop with spare cores, a Linux
box. An agent on one host should be able to start a worker on another and work with it as it
works with a worker beside it ([ADR 0012](./0012-agents-talk-to-agents.md)): send it messages,
hear its reports and questions, read what it said, wait on it, and get its work back. Each
host already listens on iroh while it is signed in ([ADR 0007](./0007-remote-access-over-iroh.md))
and lets phones of its own account in with a join ticket and its owner's approval
([ADR 0009](./0009-accounts-and-backend.md)).

A worker on another host must not run on the lead's agent sign-ins: credentials stay on the host
they were made on. And the code it works on lives in the lead's checkout, which the other host
may or may not have.

## Decision

- **A host link is its own kind of trust, beside devices.** A host joins another with the same
  join ticket a phone uses, from the same account, naming itself where the phone would be, over
  its own ALPN, `sikemux/link/1`. The other host checks the ticket itself and its owner allows
  the link once, in the app or from a phone with full control on the account, as for a phone.
  Links live in `<socket>.remote.json` beside the devices, are listed by the `pairing` call,
  and are removed the way a device is: from either host, or by removing the host from the
  account, which tells every host of it. A link is symmetric: once allowed,
  either host may ask the other things, and both keep the other as linked. A phone's access
  levels do not apply to a host; a linked host may do only what the link asks below.
- **Linked hosts ask each other things, not the core protocol.** Each ask is a QUIC stream of its
  own on one connection between the two, which either may open: a JSON control frame naming a
  method, then a git bundle's bytes when one goes with it, answered with one JSON frame. The
  methods are the link's own (spawn, deliver, read, what an agent did, questions, commits),
  answered only while the asking host is still linked. Devices keep the core protocol and its
  `Call` methods; nothing changed in `sikemux-wire` or `sikemux-client`.
- **The Mac app asks the account for a host's ticket.** The account's join route signs a ticket
  for the Mac app's sign-in only when the joiner is one of the account's hosts, and for a phone's
  sign-in only when it is one of its phones, as before.
- **The lead's host names the worker and counts the team.** `agent_spawn` with `host` names the
  worker on the lead's host, under the same caps, which count every agent of the team on every
  host; a worker elsewhere that starts its own asks the lead's host to name it. The worker's
  host starts it on its own sign-in for that CLI, never the lead's, in the lead's permission
  mode as far as the CLIs share one.
- **Code crosses as git.** The lead's host sends its commit, its branch and its `origin`, and
  what it has not committed, new files too, as a patch made in an index of its own. The worker's
  host finds the project by its `origin` and cuts a worktree beside it on a branch of its own,
  fetching the lead's commit from a bundle when it lacks it, or keeps its own copy of the
  project in its data folder. The lead's uncommitted changes go on as a first commit, so the
  worker's own work reads apart. Each time the worker's turn ends, its new commits come back as
  a bundle onto a branch in the lead's checkout named for the host and the worker, ahead of its
  report, which names the branch. Sikemux never merges it; the lead decides.
- **Each host keeps its own rules.** An agent on a linked host is known here by its host and its
  agent id and shown with the host's name, "Otter on studio". A message, report, question or
  answer for it goes to its host, which hands it on with the same rules as its own agents'
  messages: blind reviewers stay out, loops are held, reports are never refused. Both hosts note
  each message, so a loop across the link is held too. What each agent of a team does is sent to
  the hosts its teammates run on, which journal it for `agent_wait` and show it. A worker's
  question goes to its lead, and from there to the person on the lead's host; the answer goes
  back to the worker's host, which keeps a secret answer in its own private file. Which agent is
  calling stays the process-tree lookup on its own host.
- **A linked host's agents use none of this host's sign-ins until the person allows it.** The core
  keeps, for each agent, the linked hosts whose agents started it or messaged it, and an agent
  that one of those starts or messages carries them on, after it closes too. Such an agent is
  refused plugin calls and the browser tools, whose tabs hold this host's site sign-ins, and gets
  none of the environment plugins give an agent at launch, such as its AWS profile. The refusal
  names the host and says the person has not allowed it. The check sits where the core answers
  every tool call, not in each plugin. Each link has its own switch for this, off when the link
  is made and kept with it, which only the person changes: in the app's Settings, under Devices,
  or from a phone that acts as them. An agent's own sign-in for its CLI is not one of these; a
  worker still runs on this host's.
- **What must arrive waits.** Messages, reports, questions, answers and commits for a host out of
  reach wait in a bounded outbox in the data folder and go in order when it is back. Its agents
  show as out of reach, their leads hear after a few minutes and again when it is back, and a
  host whose link is removed leaves its teammates here a note rather than a silent wait.

## Consequences

- A linked host can start agents on this one with full access to the projects it shares, on this
  host's agent sign-ins, in permission modes as wide as its own agents'. That is why the owner
  allows each link and can remove it at once. Without the person's word, those agents cannot act
  as the person in a plugin, a database or a signed-in tab here, so an agent tricked by what it
  read on one host cannot reach the other's services.
- A lead whose worker runs on another host hears its reports, so it too is refused this host's
  sign-ins until the person allows the worker's host.
- The check covers what Sikemux hands agents: plugin calls, the browser tools and the launch
  environment. An agent that runs without approvals still has its shell, so sign-in files the
  host's command-line tools keep, such as `~/.aws`, `~/.ssh` and `gh`'s, stay within its reach.
  Only the per-agent sandboxes of
  [ADR 0014](./0014-agents-run-on-hosts-and-every-client-is-a-way-in.md) close that. Until then
  the per-host switch is the trust line: a host whose agents the person would not let read those
  files should not be linked.
- Starting a worker on another host sends the lead's code to it. A large repository the other host
  lacks takes its first bundle's time; a later worker for the same project sends only what is new.
- A worker's checkout stays on its host after it closes, as a worktree beside the project or under
  the data folder.
- Moving a running agent to another host, and picking the host with spare cores, are left for
  later.
- Only the Mac app asks the account for a host's ticket today, so a host without a window is
  linked by a host with one joining it; the link then works both ways.

## Alternatives considered

- **Linked hosts as paired devices with a new access level.** Rejected: a device is a client of a
  core over the core protocol, and the access levels and `Call` methods are the phone's contract.
  A host link is two cores asking each other, both ways, for things no phone asks.
- **Sharing the lead's sign-ins with the other host.** Rejected: credentials never leave the host
  they were made on.
- **Pushing through the project's own remote.** Rejected: it needs network access and permission
  to push, publishes work in progress, and fails for a project with no remote.
- **Running the worker's tools on the lead's host.** Rejected: the worker needs its own files,
  processes and browser where it runs.
- **Letting each plugin decide whether a linked host's agent may use it.** Rejected: one plugin
  that forgot to check would hand out its sign-in, and the core already answers every tool call.
- **Refusing only the agents a linked host started, not those it messaged.** Rejected: a message
  steers an agent as well as a prompt does.
- **Hiding the host's sign-in files from a linked host's agents with a profile around their
  process** (`sandbox-exec` on macOS, a mount namespace on Linux). Rejected: a determined agent
  gets its commands run outside the profile, so it guards only against reading them by accident,
  and macOS refuses a sandbox inside another, which breaks Codex's own sandbox in Normal mode.
  The sandboxes of ADR 0014 replace it.
