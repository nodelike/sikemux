# ADR 0014: Agents run on hosts, and every client is a way in

- Status: Accepted
- Decision date: 2026-10-16

## Context

Sikemux started as an app on the person's laptop, and its agents lived there. The core already
outlives the window ([ADR 0006](./0006-background-core.md)), phones join a host over iroh
([ADR 0007](./0007-remote-access-over-iroh.md)), and hosts on one account link to run agents for
each other ([ADR 0013](./0013-hosts-link-to-run-agents-for-each-other.md)). But a laptop sleeps,
leaves the network and runs out of memory, so agents that work unattended and wake on events
cannot live there.

Where agent tools are going points the same way: agents run on an always-on machine, and laptops
and phones are windows onto it. Anthropic, Cursor and Perplexity each give every agent its own
sandbox and share work at commits.

## Decision

- **Development is driven from hosts, not laptops.** Agents run on hosts unattended and start
  work from events: a webhook, red CI, a new ticket, an alert, a schedule. Nobody has to be
  watching.
- **Every core is a host.** A host is anything running the core: a Linux server, a Mac mini, a
  cloud server, the person's own laptop. Each runs the same core and links to the others on its
  account as ADR 0013 does. The core runs without a window and installs on Linux on its own.
- **The person is one participant, joining from any device.** The web app at app.sikemux.com, the
  phone app, the desktop app and the notch are equal ways in to any host on the account. None of them is
  where the work happens, and feature logic a client needs lives in the core or in TypeScript the
  clients share.
- **Each piece of work runs in a sandbox of its own.** The core starts work through a sandbox
  backend with two kinds: a git worktree on the host, as today, and a microVM through
  [SPACE](https://github.com/perplexityai/space). SPACE suspends a sandbox with its memory while
  the agent waits and wakes it on the next event, copies a running sandbox cheaply, and keeps
  credentials outside it.
- **Events start work, the board holds it, Auto carries it.** An event becomes a card; Auto runs
  the card in a sandbox; the blind reviewer works in a copy of it; the work lands as a commit or
  pull request; checks run after it lands; the inbox and digest reach whichever client the person
  has open.
- **Agents never hold service credentials.** Tokens for GitHub, Jira, AWS and the other plugin
  services are added to requests outside the sandbox, so an agent can use a service but cannot
  read or leak its token.
- **Sikemux runs no fleet of its own.** Hosts are the customer's machines, or SPACE run by a
  provider that offers it.

## Consequences

- A Linux package of the core alone, with no window and no WebKit, and a one-line installer come
  first: without them a server cannot be a host.
- Agents in sandboxes of their own share work at commits, through the check that a commit builds
  on the current head and a replay when it does not, as `git_commit` already does. Per-line
  attribution stays for provenance; warnings between agents in one shared checkout matter less.
- An agent waiting on CI or a person costs nothing while suspended, but only once suspend and wake
  times are measured on real work. Sikemux's own repository is run this way first.
- SPACE needs KVM, so a SPACE host is bare metal or a provider's SPACE, not an ordinary cloud
  virtual machine. Hosts without KVM keep the worktree backend.
- Unattended agents on machines the person does not control run on API keys and coding plans. A
  person's own subscription runs only on a machine they control, signed in through the vendor's
  own flow on that host.
- The Mac app becomes a client of any host on the account rather than only its own core, and the
  web gains a client beside the phone. How the web client reaches a host is decided when it is
  built.

## Alternatives considered

- **Keeping agents on the person's laptop, with teammates taking over at checkpoints.**
  Rejected: unattended work stops whenever a lid closes.
- **Running Sikemux's own fleet of machines.** Rejected: memory prices, idle machines and
  hypervisor work cost more than the margin.
- **Every agent in one shared checkout on the host.** Rejected as the default: sandboxes can be
  suspended, copied and thrown away, and a commit is a clear point to share work. It stays
  possible with the worktree backend.
- **A hosted sandbox service only (E2B, Daytona, Fly Sprites).** Not chosen as the base: each puts
  every host on one vendor's machines, while SPACE also runs on the customer's own.
  SPACE also answers E2B's API, so a hosted service can stand in where it fits.
