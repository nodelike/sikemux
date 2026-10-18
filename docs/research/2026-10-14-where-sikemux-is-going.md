# Where Sikemux should aim — research synthesis, 2026-10-14

Research passes: the 9 linked posts and article, the Anthropic Scion article + HN, a competitor map
(~45 products), source control / review futures, agent-usage futures, business models, an inventory of
Sikemux itself, and a second round on T3 Code and herdr / Herdr UI, the open-model vertical, always-on
cloud machines, and Amoeba. Many funding/ARR numbers come from trackers and secondary blogs; treat them
as approximate.

Updated 2026-10-15. Since this was written, the core started running a host on its own, the model
layer's first four items, the ranked inbox, per-line agent attribution with `git_commit`, and Auto's PR
review loop were built on `main`. Section 13 audits agent-to-agent as built.

Updated 2026-10-16. v0.5.3-nightly.2 shipped on 2026-10-15 with all of the above, plus hosts that link
and run workers for each other (ADR 0013), collision warnings and resume at a usage-limit reset. Section 0
records where Sikemux is now headed; where it disagrees with later sections, section 0 wins.

---

## 0. Direction, 2026-10-16: server-driven development

Sikemux is headed to development driven from servers, not laptops. Agents run unattended on servers and
act on events: a webhook, red CI, a new ticket, an alert, a schedule. Nobody has to be watching.
[ADR 0014](../architecture/0014-agents-run-on-hosts-and-every-client-is-a-way-in.md) records the decision.

- **Every core is a host.** A host is anything running the core: a Linux box, a Mac mini, a cloud
  server, a laptop. Hosts on one account link, as ADR 0013 already does.
- **The person is one participant, joining from any device.** The web app (app.sikemux.com), the phone
  app, the desktop app and the notch are equal ways in. None of them is where the work happens.
- **Each piece of work gets its own sandbox.** Perplexity's SPACE (Apache-2.0, Rust, Firecracker
  microVMs) is the candidate: it suspends a sandbox with its memory while an agent waits and wakes it on
  an event, branches a running sandbox cheaply, and keeps credentials outside the sandbox so an agent can
  use a service without ever holding its token. It needs KVM: bare metal, or SPACE managed by CoreWeave,
  Akamai or Crusoe. The local worktree stays as the other backend.
- **Events start work, the board holds it, Auto carries it.** Signal → card → sandbox → agent → evidence
  → blind review → PR → merge → checks after landing → inbox and digest on whichever client the person
  has open.

What this changes below:
- Focus 1 (Host) and section 6's routines and webhooks become the core of the product, not catch-up.
- Agents in their own sandboxes share work at commits through the stale-base check and replay that
  `git_commit` already does. Per-line attribution stays for provenance; collision warnings and leases in
  one shared checkout drop in priority.
- Unattended agents on servers Sikemux or a provider runs use API keys and coding plans; a person's own
  subscription only on a machine they control. The model layer (focus 2) is how they get paid for.
- Still no fleet of our own (section 7): the customer's machines, or managed SPACE.

Order:
1. A core-only Linux package and one-line installer, with agent sign-in on the host.
2. A sandbox backend in the core: worktree or SPACE, starting with SPACE's direct mode on one KVM box.
3. Auto runs cards in sandboxes: suspend while waiting on CI or a question, wake on the event; the blind
   reviewer in a sandbox branched from the card's.
4. Events in: a webhook endpoint on the host, schedules, CI, Jira, Linear, Slack, alerts.
5. Credentials added outside the sandbox for plugin services, never in the agent's environment.
6. A web client on app.sikemux.com, and the Mac app as a client of any host.
7. Hosted for teams on managed SPACE, as the Team tier.

Run Sikemux's own development this way first, to measure suspend and wake times before promising them.

---

## 1. What the links are actually saying

| Post | Signal |
|---|---|
| Owen Gretzinger: "is anyone building a good cloud agent platform" + screenshot of T3 Code at 62 GB, 30 GB swap | Laptops have hit a physical ceiling for agent fleets (client app + the dev servers agents spawn). Replies were wall-to-wall vendor pitches (Devin, Factory Environments, Amp orbs, Omnara...). |
| Bruno Santos: tried Orca, T3 Code, many GUIs, keeps going back to herdr | Speed and lightness decide GUI-vs-TUI. Winning screen: agent ↔ worktree ↔ PR → CI → merge → deploy on one card. |
| Fabien Penso: Herdr UI release (Teleport worktrees to/from cloud, SSH devices) | Native Rust/GPUI client for herdr: browser annotate → agent, diff notes → agent, per-machine CPU/mem, plan-usage meter. Teleport moves a worktree *with running programs and agent sessions* between hosts. No chat UI. |
| Elias Stråvik: herdr-projects | Coordinator that never works itself; workers in worktrees; blind reviewer with a rubric the maker never sees; PR ratchet; "needs you" board. |
| Rafal Wilinski: Runlayer "software factory" | Work starts from signals (calls, Slack, red CI, pins on prod via Agentation) not prompts; sandbox → evidence (before/after, video) → adversarial review → babysitter loop → automerge → QA fuzzer → digest. 91 PRs merged by one agent in 24h. New bottleneck: humans knowing what shipped. |
| Anthropic: Redesigning source control (Scion) | 2.5k → 30k merges/day. Server-side commits, lock-free CAS landing, no merge queue, fix conflicts after the fact, review by *intent* not lines. Explicitly leaves "agents conflicting with each other locally" unsolved. |
| Karpathy: the funnel | 0.2% of people live in the agent world; at the frontier "human review and comprehension are starting to fall behind." |
| Julius (T3 Code): cross-env orchestration | An agent on the laptop delegates a subagent to "box", server-to-server, with lineage. |
| DHH: "every developer needs an AI shed" | Always-on box on tailscale runs the (herdr) agents; laptop and phone are windows; close the lid without breaking anything. Most-requested addition in replies: a "what finished, what's running, what needs me" handoff. |

**The two currents underneath all of it**

1. **The agent runtime is leaving the laptop.** Daemon + thin clients (herdr, shed, Teleport, T3 box, every cloud vendor).
2. **The bottleneck is now human attention and trust, not agent throughput.** Intent review, evidence, blind reviewers, gates, digests, "needs you".

---

## 2. The market and who Sikemux actually competes with

Parallel sessions + worktree per task + diff review + browser pane + phone remote + notifications is now
commodity. 100+ apps do it; Orca hit ~90k stars giving it away under MIT; vibe-kanban and Terragon died
with no business model.

There are two different verticals, and Sikemux is only in one of them:

- **Single vendor, single model family.** Claude Code desktop, Codex in the ChatGPT app, Cursor 3, Copilot
  app, Antigravity. Rich and free with the plan, but they only run their own agent. They take people who
  will only ever use one vendor; that person is not Sikemux's user.
- **Every harness, every model.** The user picks the agent (Claude Code, Codex, OpenCode, Hermes, Pi, Grok,
  OMP, Muse...) and the model (frontier, open, local) per task. This is Sikemux's vertical.

**Direct competitors in that vertical:** T3 Code and herdr + Herdr UI (details in section 9). Behind
them: Orca, cmux, Conductor, Superset, JetBrains Air, Zed (ACP). Amoeba competes with the future Team tier,
not with Sikemux today (section 11).

---

## 3. Where Sikemux stands against T3 Code and herdr

**Sikemux is ahead on**

- **Native chat for every harness** with steering and rewind. herdr and Herdr UI have no chat at all
  (terminal-screen scraping for state). T3 has chat but runs it through an Electron app with known memory
  blowups (63 GB after sleep, 21 GB GPU leak) and is slower than Codex alone on the same task (#695).
- **Weight and stability:** 16 MB native-feeling app against Electron (T3) and a GPUI painter its author
  says "can consume significant CPU" (Herdr UI).
- **Phone app over iroh** with host approval, no open ports, no tunnel, no environment cap. herdr has no
  first-party phone app; T3 Connect is a Cloudflare tunnel with a 3-environment cap on free accounts.
- **The core already keeps agents alive** across window quit and app updates.
- **Ops context agents can use:** Jira, Slack, SigNoz, AWS, Rundeck, DB client, three code hosts; iOS
  Simulator tools; notch island; on-device dictation; encrypted shared chats.
- **Many agents in one checkout**, on purpose (both rivals default to worktrees).
- **Named agents, @mentions, agent-to-agent** built (ADR 0012): spawn/send/read/wait, questions that go
  to the lead first, a board per project with Auto, workers listed under their lead on the rail, phone
  and notch. Teams mix vendors and models, and terminal agents take part. herdr has no parent/child
  tracking; T3 V2 has `delegate_task` and lineage across machines, so this lead holds on one host only.
- **A core that runs without the app.** It keeps projects, launchers and agent accounts, finds agents
  itself, and starts and wakes chats for a phone with no window open.
- **Every line knows its agent.** The core records each chat agent's edits, the git panel shows who wrote
  each block, and agents commit only their own lines with `git_commit`.
- **One ranked inbox** on the rail, notch and phone, with a handoff note, the browser recordings an agent
  made, and a "while you were away" digest.
- **A model layer.** Claude Code and Codex chats run on a connected provider, coding plan or local runner.
  Each model's tool calls are checked, and helpers run on a model of their own.

**Sikemux is behind on**

- **Multiple machines.** herdr has `machine add` over SSH; Herdr UI has Devices, Smart Dispatch, Teleport
  (worktree + running commands + agent sessions moved between hosts), Coder/Daytona/Railway cloud
  devices; T3 has one server per machine with clients attaching to many, plus cross-machine
  `delegate_task`. Sikemux's phone already keeps several hosts, but the Mac app talks only to its own
  core.
- **PR loop.** T3: multi-host PR pages, guided reviews, agents woken on checks/reviews/conflicts. Herdr UI:
  orchestrator tab that dispatches issues to agents, reviews and merges PRs. Sikemux's board Auto opens
  the PR, waits for CI, gets one fix, hands review comments to the agent, replies in each thread and asks
  for review again. Missing: PR pages across hosts, guided review, waking on merge conflicts.
- **Automations:** T3 has scheduled and webhook tasks and auto-resume after a usage-limit reset. Sikemux
  moves a chat to another signed-in account at a limit; resume at reset is in progress; no schedules or
  webhooks.
- **Outside agents driving it:** T3 exposes an MCP server with OAuth so ChatGPT or Claude can drive threads;
  herdr's whole CLI and socket API are scriptable, which is why it has ~1,700 community plugins.
- **Distribution and openness:** herdr 43.8k stars (Apache-2.0, $6M seed from Bessemer/YC, one founder),
  T3 27.4k stars (MIT, ~3.5k stars/month), both releasing every few days. Sikemux is FSL and absent from
  every public comparison found.
- **Reach:** Sikemux ships deb, rpm and AppImage but no Windows build; herdr and T3 run on both. herdr
  lists 23+ agents.
- **Shipping.** Everything above that is new since 2026-10-13 is on `main` but in no release.

---

## 4. Thesis

> **Sikemux is the control room for your agents: every harness, every model, every machine you own, and
> one place that tells you what needs you and why to trust what they did.**

Don't fight on parallelism or panes; that's free everywhere. Fight on three things:

1. **Every harness, every model** — not just running any CLI (herdr, Orca do that) but owning the model
   layer underneath them.
2. **Where agents live** — your hosts, not the laptop, without SSH config or tunnels.
3. **How a human stays in charge of 20 of them** — attention, provenance, evidence, undo.

All three build on things only Sikemux has: the core daemon, native drivers for many harnesses, the
provider key vault, iroh, the shared checkout, the phone and notch.

---

## 5. What to focus on, ranked

### 0. Get the built work to users
Done for v0.5.3-nightly.2 (2026-10-15). Keep shipping: what landed since, such as signed agent footers
and changes made outside the edit tools being recorded, goes out in the next nightly.

### 1. Sikemux Host — the shed, without the setup
Run the core headless on Linux or a spare Mac mini, install with one command, join it to the account with
a join ticket. The Mac app and the phone become clients of **several hosts**, each shown as a card with
CPU/memory and its agents. Agents keep working when the lid closes.
- **Done: the core runs a host on its own.** It keeps projects, launchers and agent accounts, and starts
  and wakes chats without the app. A host with no browser signs in by code and approves it on
  app.sikemux.com. `sikemux core pair` and the owner's phone let devices in. Linux has a systemd user
  service, host info and CI tests.
- **Left:** a package for the core alone with no WebKit, and a one-line installer. Agent sign-in on the
  host, relayed to the Mac or phone (#75, #76). Surviving network drops (#106), which matters more once
  the agents are remote.
- **Phone first.** The phone already pairs with several hosts and shows a card per host, so Host can ship
  to phones before the Mac app becomes a multi-host client, which is the bigger job (one local socket
  today, and git, previews and files assume a local disk).
- iroh means no open ports, no Tailscale, no SSH config: simpler than herdr's `machine add`, and no
  environment cap like T3 Connect. Ship this before Herdr Cloud leaves its waitlist.
- Agent logins happen **on the host**, with the login URL/code relayed to the Mac or phone. Never copy
  `~/.claude` or Keychain credentials across (Anthropic forbids intermediating credentials).
- Next: move a project and its live agents between hosts (Teleport), cross-host `agent_spawn` with
  lineage, and "send this to the host with spare cores".
- Answers Owen's memory wall: dev servers and builds run on the box.
- Cloud plan in section 7.

### 2. Every harness, every model — own the model layer
herdr and Orca run any CLI but know nothing about models. T3 brings your subscriptions but has no
open-model or local story. JetBrains Air supports local models through a hand-edited JSON file. Nobody
ties keys, coding plans and local models together and configures each harness for them. Sikemux has most
of it now. The catalogue is data in the core, with coding plans (z.ai, MiniMax, Kimi Code, OpenCode Go,
Synthetic, Ollama Cloud) and local runners (Ollama, LM Studio). Claude Code gets `ANTHROPIC_BASE_URL`,
Codex a provider of its own, and the others env vars. The picker says who pays. A short probe marks each
model "works with agents" (nobody else ships that check). Helpers, workers and Auto can name their own
provider and model.

Left, ranked:
1. **One meter across plans, with fallback.** Show every plan's 5-hour and weekly windows next to
   pay-per-token spend, and offer "this plan is out, continue on X".
2. **One-click local.** Detection is done. Left: suggest a model by RAM (Qwen3.6-27B for 48–64 GB,
   Qwen3-Coder-Next or Qwen3.6-35B-A3B for 64 GB+), pull it, probe it, wire it in.
3. **A no-account first run** through OpenCode's free models or OpenCode Go. Use OpenCode; don't build a
   harness.
4. **A cheap default for helpers.** The setting exists; ship a sensible default so scouts and reviewers
   run on a cheap open model and the builder keeps the frontier one.

Why it is real: Chinese open models are about half of OpenRouter tokens; Hermes (1.99T tokens/day),
Kilo, Cline, pi and omp all outrank Codex on OpenRouter; open models cost a quarter to a sixth of frontier
and sit roughly 6 points behind on overall indexes. People switch for cost, rate limits, privacy and
vendor-policy risk. Pitfalls: model names and plan limits change monthly (keep the catalogue as data),
plans pause sign-ups when GPUs run out, don't publish benchmark claims without your own runs, and offer
US/EU-hosted copies of Chinese weights for companies.

### 3. Many agents, one checkout, every line knows who wrote it
Nobody else has this, and Anthropic's article leaves it open. Sikemux has the first half. The core records
every chat agent's edits by line, works out who owns each uncommitted line, and the git panel shows it.
Agents commit only their own lines with `git_commit`, through a private index and a compare-and-swap on
the branch. Claude chats are refused `add -A`, `commit -a`, `stash`, `reset --hard`, `checkout .` and
`clean -f`.
Anthropic (Scion), Cursor ("Git at any scale") and Perplexity (Projects, section 12) all landed on the
same commit layer: one shared head, accept a commit only if it builds on that head, make the loser replay
and resubmit, no pull requests, a log of who changed what. All three give each agent its own sandbox and
share at commits. Sikemux shares the working tree itself, which is the part none of them solve.
- **Close the holes.** The git guard now covers every agent and transport.
  Terminal agents' edits and Bash edits are still not recorded; a file changed outside the record loses
  its owners instead of being marked unattributed.
- **Collision warnings before commit:** two agents in one file/hunk → both told via agent_send; short
  leases on lockfiles, migrations, generated code. Block a stale full-file write that would erase
  another agent's edit (the failure every shared-checkout write-up names).
- **Per-agent undo:** snapshot each agent's turns under a private ref so "undo everything Otter did since
  14:02" leaves the others alone. Covers what Claude's /rewind misses (bash, other CLIs).
- **Provenance:** trailers name the agent, CLI, model and chat. Left: the card, a click from a commit or a
  changed block to the transcript turn, an outbox of accepted commits (section 12), and Agent Trace
  records. (Agent identity only — no Claude co-author lines.)

### 4. The decision inbox (one surface for rail, notch and phone)
Replace notification streams with a ranked queue: blocked/asking agents first, changes to review next,
finished work, then progress. Batch on your schedule; show a work-in-progress count; write a handoff note
when an agent stops. A "while you were away" digest on open.
- Done: the core ranks one inbox (asking, then review, then finished, then working) for the rail, notch
  and phone. Each item has a handoff note and the browser recordings from that turn and card. A "while
  you were away" digest opens on the phone and the rail. Permissions, questions and assumptions are
  answered in place.
- Left: a risk class per change, rules that escalate the risky ones, batching on your schedule, and
  revert from the phone.
- Review by **intent + evidence**, not lines: what was asked, what the agent says it did, risk class
  (comment/test-only vs logic vs migration/protocol/server), checks that ran, and the browser recording or
  before/after screenshot it took (browser_record already exists). Diff last.
- Rules escalate the risky classes; routine changes with evidence pass. Approve / revert / steer from the
  phone in one tap.
- T3 is investing heavily in PR review; the difference here is local attribution plus evidence, across
  harnesses, before anything is a PR.

### 5. Make review cross-vendor, on top of agent-to-agent
- ADR 0012 is built, and the audit's problems are fixed (section 13). Left there: spawn across hosts,
  collision warnings, and secrets out of agents' environments.
- Linear cards (#67; only Jira feeds the board). The adapter for agents outside Sikemux stays deferred.
  Agent-to-agent across hosts waits on focus 1.
- **Done: a blind reviewer before Auto merges.** An agent on another CLI, seeing only the card, its
  acceptance criteria and the diff, must approve the exact commit. Left: an on-demand version any lead
  can call outside Auto, and a first real run against a pull request.
- **Definition-of-done gates.** Auto runs a project's check command before it lands a card with no CI.
  Make it a gate any agent's card must pass, with acceptance criteria on cards and hooks that reject and
  say why. Huntley: enforcement beats rules files.
- After landing on main: run affected checks; if main goes red, bisect to the agent and commit and tell
  that agent. "Revert Otter's last commit" without touching anyone else.

### 6. Close the parity gaps against T3 and Herdr UI, cheaply
- Done: Auto hands review comments to the agent, replies and asks for review again.
- Resume when a usage limit resets (in progress), then routines and webhook triggers on your own host.
- An MCP/CLI surface outside agents can drive (T3's `/mcp` with OAuth). Plugins stay compiled in
  (ADR 0002); this is about being drivable, not a marketplace. It needs the window-identity fix in
  section 13 first.
- Signals → board: Jira is in. Add Linear (#67), Slack threads and red CI.
- Finish #71 (install and sign-in, #74–#77) and #106 (#107–#111).
- Publish memory and CPU numbers with 20 agents open; it is T3's weakest point and herdr's open complaint.

### Stop or don't start
- Running your own cloud fleet, or reselling inference or coding plans.
- A skills marketplace (90k+ low-quality skills on skills.sh; malware via ClawHub).
- A standalone AI review SaaS (CodeRabbit, Graphite/Cursor own it); keep review inside the app.
- Worktree wars and Best-of-N as the headline; keep worktrees as an option for the rare real conflict.
- Leading with the notch or the phone remote alone; keep them as the delivery surface for the inbox.
- Building your own agent harness; drive OpenCode, Pi, Hermes and the rest.
- The GPUI rewrite (#115) stays parked; bounded memory matters more than the renderer right now.

---

## 6. Products to add

| Product | What | Price |
|---|---|---|
| **Sikemux** (Mac, Linux) | The whole local app, every harness and model, unlimited agents, phone on one host, push | Free forever: it's the distribution |
| **Sikemux Host** | Headless core for a Linux box / Mac mini / VPS; one-line install; one-click into your own Hetzner or DigitalOcean | Free for one host |
| **Sikemux Pro** | Multiple hosts per account, moving work between hosts, synced settings/skills/provider config, history search across machines, routines on your hosts, the cross-plan meter | ~$10/mo or $96/yr (Termius model: ~$4M on ~2M users charging for sync + devices) |
| **Sikemux Team** | Live shared sessions (join, steer, take over at a checkpoint), shared skills and provider setups, team inbox, roles, central billing | ~$20/seat/mo — the most likely thing a company pays for |
| **Later: Sikemux Shed** | A managed always-on box on Fly Sprites, opt-in, card required | ~$25/mo for 120 active hours, $0.20/h after (section 7) |
| **Later: review check** | The same intent + evidence + adversarial review as a GitHub/Bitbucket check for teams | Team add-on |
| **Later: pin-to-agent widget** | A small script for staging/prod pages: click an element, leave a note, it lands as a card for your agents (Agentation-style) | Team add-on |

Referral deals with inference providers (z.ai pays in credits, OpenRouter has no affiliate programme) are
for co-marketing, not income. Optional later: require Pro/Team for commercial use above ~10 people once
there is real usage at companies.

---

## 7. Cloud: should users get VMs for an always-on machine?

**Not first. Phase it.**

The tools closest to Sikemux (herdr, Herdr UI, T3, Superset, Cursor's "My Machines") connect machines the
user already owns. The funded seat-priced products (Conductor, cmux, Amp, Factory) host. Anthropic and
OpenAI now include cloud VMs free with the plan, so hosted compute for Claude/Codex users competes with
free. Herdr Cloud (waitlist) only links your own machines; it does not host agents.

**Phase 1 — bring your own box (now).** The Host in focus #1. "Use this Mac mini as a shed" covers
macOS/iOS work. The core already builds, tests and runs as a service on Linux. Left: a core-only package
and an installer (#10).

**Phase 2 — one click into the user's own cloud account.** Hetzner project token or DigitalOcean OAuth,
cloud-init installs the core with a join ticket. The token stays in the Mac Keychain, used once, never sent
to the accounts server. Default to Hetzner CPX32 (4 vCPU / 8 GB, about €36/mo after the 2026 price rises).
Optional "sleep when idle": snapshot and delete through the user's token, recreate on wake. Sikemux pays no
compute bill, handles no abuse or KYC, needs no extra vendor terms, and each user keeps their own IP.

**Phase 3 — managed "Sikemux Shed", only on demand.** Trigger: waitlist at ~10% of weekly active desktop
users, or proof people will pay.
- Run it on **Fly Sprites**, which bill CPU and memory actually used ($0.0385 per CPU-hour,
  $0.021875 per GB-hour), checkpoint in about a second, and don't meter bandwidth. Agents spend most of
  their time waiting on the model, so this is the only billing shape that fits. E2B/Daytona/Vercel bill
  reserved capacity (a 4 vCPU / 8 GB box is ~$0.33/h busy or not).
- Price ~$25/mo for 120 active hours of 4 vCPU / 8 GB + 50 GB, $0.20/h after, user-set hard cap, card
  required, no free tier. Cost per box ~$12 (light) to ~$20 (heavy compiles): 20–50% gross margin.
- Before launch: accept Anthropic's Commercial Terms (preinstalling or running Claude Code in hosted
  sandboxes requires them); make the accounts server wake a sleeping box when a phone connects; check that
  the core's open connections don't stop Sprites from ever sleeping; DPA and subprocessor list; mining and
  egress guards.

**Never — own fleet.** 2026 RAM prices, idle-cost exposure and hypervisor work cost more than the margin.

Logins on remote boxes: Claude Code with the user's own subscription on a box they control is allowed
("including where a platform hosts Claude Code"); sign-in must go through Anthropic's flow. Codex supports
`codex login --device-auth` (beta, enabled in ChatGPT security settings first).

---

## 8. Platform risk rules

- Never touch model credentials; launch vendors' own installers and logins (#71's stance is right).
- Keep the terminal transport fully working for every agent. If Anthropic revives metering of
  `claude -p` / stream-json for third-party apps, chat panes are what gets hit.
- Apply to OpenAI's "Sign in with ChatGPT" plan-usage programme for coding tools (live since 2026-09-29;
  Conductor, Warp, Amp, OpenCode, Hermes are in). It's permission and a listing.
- Support API keys, coding plans, open and local models alongside subscriptions; that is the hedge and the
  vertical at once.
- Say "runs Claude Code"; never put Claude/Anthropic in names or marks.
- Security worth doing while keeping the YOLO default: agent browser tabs on their own profile with no
  user cookies unless granted per site (agent browser + your sessions = the "lethal trifecta"), an audit
  log of agent-to-agent messages, per-turn snapshots.

---

## 9. T3 Code and herdr / Herdr UI in detail

**T3 Code** (pingdotgg/t3code, Theo and Julius)
- 27.4k stars, MIT, Electron + Node server (Effect-TS), React web app at app.t3.codes, Expo phone apps.
  Growing ~3.5k stars/month; nightly builds daily, a stable release every week or two.
- One server per machine; web, desktop and phone clients attach over authenticated RPC. Reach it directly,
  over Tailscale, SSH, or T3 Connect (Cloudflare tunnel + Clerk; free accounts capped at 3 environments).
- Harnesses: Codex (app-server), Claude (Agent SDK), Cursor SDK, Grok, OpenCode, Antigravity, Pi, Muse,
  ACP registry. Bring your own subscription; open models only through OpenCode/OpenRouter/Pi.
- Orchestrator V2 (event-sourced on SQLite, nightly only): `delegate_task` to any provider or machine,
  lineage view, server-side steer/queue, fork and rollback, scheduled and webhook tasks, auto-resume after
  limits.
- Heavy PR investment: multi-host PR pages, stacks, guided reviews, PR watches. Agent-drivable browser,
  including one running on the remote server. MCP server for outside agents.
- Weak spots: memory (63 GB after sleep, 21 GB GPU leak, idle server growth), slower than Codex alone
  (15–20 min vs 4.5), Claude usage 3–5x higher than the native app (closed), telemetry on by default
  despite promising otherwise, V1→V2 migration loses checkpoints, no paid tier.

**herdr** (herdrdev/herdr, Can Celik)
- 43.8k stars, Apache-2.0 (was AGPL until July), Rust + vendored libghostty, ~1.4M installs, YC F26,
  $6M seed (Bessemer lead; Tobi Lütke, Dane Knecht among angels). Mostly one founder.
- A background server owns the PTYs; thin TUI clients attach; newline-JSON socket API and a CLI that is
  also the plugin API. ~1,700 community plugins (34 with 100+ stars): phone/web clients, reviewers,
  coordinators, browsers.
- 23+ agents detected by reading the bottom of each pane's screen. No chat, no ACP, no model layer, no
  built-in browser or review. Worktrees first-class.
- `machine add` over SSH since 0.9.0; agents can't see agents on other machines yet. Herdr Cloud
  (waitlist) will link your own machines with end-to-end encryption, not host agents.
- Complaints: lag with many agents, server dying after the last client detaches, SSH drops killing remote
  panes, no auto-naming.

**Herdr UI** (penso/herdr-gpui, Fabien Penso — not affiliated with herdr)
- 1.3k stars in three weeks, Apache-2.0, Rust on pre-release GPUI, macOS/Linux/Windows. A side project.
- Paints herdr's terminal cells natively. Review Changes with line notes to the agent, checkpoints,
  WebKit browser tabs with annotate-to-agent, plan usage meter, Orchestrator tab (issues/PRs → dispatch,
  review, merge).
- Teleport: worktree (git bundle), uncommitted changes, layout, running commands and agent sessions
  (Claude Code, Codex, opencode, pi/omp, Copilot) moved to another host; others get a handoff note.
  Smart Dispatch to the host with spare cores; Devices overview; Coder/Daytona/Railway cloud devices.
- No chat, no phone app. Admits its cell painter can use significant CPU.

**What they'll likely ship in 3–6 months:** T3 V2 to stable with store phone builds, more PR review,
fleet management across machines, more providers. herdr: Herdr Cloud, agents that see each other across
machines, more clients beyond the TUI, Windows parity, toward 1.0. Herdr UI: device setup, cloud devices,
moving single panes between devices.

**The five moves that beat both:** Host over iroh before Herdr Cloud (focus 1); the model layer neither
has (focus 2); native chat for every harness, kept fast and bounded on memory (their weakest points);
attribution, inbox and evidence (focus 3–4, first versions built) against T3's PR-centric review;
agent-to-agent (built on one host) extended across hosts with lineage, before T3 V2 reaches stable.

---

## 10. Claude Code desktop and the other single-vendor apps

Not the main competitor: they sit in the single-vendor vertical. Worth watching because they set user
expectations for free (browser pane, iOS Simulator pane, SSH, cloud sessions, cross-session messaging,
Remote Control, Dispatch) and because Anthropic's policy on third-party use moves often (banned in
February, reinstated in May, metering paused in June, plan credits added in October).

---

## 11. Amoeba (useamoeba.com)

Not an always-on cloud machine. A free, early-access multiplayer IDE (VS Code fork on Electron, 573 MB DMG,
~1 GB AppImage) where teammates share a live session, each with their own agent in a lane, coordinated by a
hosted server ("Brain") that stores sessions, plans, locks and transcripts with keys Amoeba holds.
Agents run on each person's own laptop; when a laptop closes or hits a usage limit, a teammate takes over
at a checkpoint. Claude Code and Codex via hooks + MCP, plus its own harness with 180+ models; managed
models at cost + 5%. About 5,400 installer downloads, ~540 Discord members, no funding found.

It competes with the planned **Team** tier, not with single-player Sikemux. Sikemux is ahead on keeping
agents alive with the lid closed, the phone, native drivers for many harnesses, the agent-driven browser,
weight, and privacy (P2P and sealed shared chats vs a vendor-readable server).

Ideas worth taking for Team: show whose account pays before a turn runs; take over at a safe checkpoint
with one winner if two click; overlap checks plus advisory locks on exact paths; memories pinned to a file
that go stale when the code moves; approvals enforced on the machine that runs the action; viewer /
commenter / editor / owner roles. (Done: chats and cards say who pays.)

---

## 12. Perplexity Projects: a Git forge for agents

Perplexity (2026-10-12; Aravind Srinivas: "a Git forge for agents, without the GitHub bloat, since no
human has to approve anything") backs every Project in Perplexity Computer with a Git repository. It is
for knowledge work in general, not only code.

- Each agent gets its own short-lived sandbox with a local checkout, managed by a small CLI that commits
  after each meaningful unit of work. Repos are bare Git on S3-backed storage (AWS S3Files), served by
  stateless replicas in isolated cells.
- Concurrency: a submit is accepted only if it builds on the current head; otherwise the agent gets a
  stale-base response, replays its change and resubmits. Fewer than 2% of submits collide; median submit
  1.7 s, p99 under 8 s.
- No pull requests. A repository "contract" is checked mechanically at submit (size, large files through
  LFS), old history is compacted into a snapshot, and an outbox logs each accepted commit with user and
  changed paths for downstream workers.
- Large files arrive as pointers with their size; PDFs and Office files are parsed into text kept outside
  the repo, so agents read the text and download the original only when needed. A FUSE virtual
  filesystem was tried and was about 400 times slower.
- Agents search with plain `ls` and `grep`: 0.627 answer accuracy against about 0.37 for semantic search
  on 150 financial-document questions.
- They rejected one long-lived shared sandbox per project (sized for peak, months of uptime).

Replies were mostly about the grep result; the open question raised was the one that matters for
Sikemux: "How do you catch two agents making incompatible changes that don't produce a merge conflict?"

What it means for Sikemux:
- A third lab confirms focus #3's commit design: stale-base check, replay, no PRs, an outbox of who changed
  what. The safe commit tool is built this way. Add the outbox next and let it feed provenance and the
  inbox.
- Their "contract checked at submit" is the definition-of-done gate in focus #5.
- Real files plus grep beat an index. Don't build a semantic index for agents; give them files, and text
  extracted from PDFs and Office files next to the originals (the PDFKit and Quick Look previews only
  display them today, so extraction would be new).
- They are cloud-only and run only Perplexity's own agents. Sikemux can offer the same shape locally and
  across harnesses, and goes further by sharing the working tree, which is why attribution and collision
  warnings matter.
- Agents beyond code, sharing a workspace with people, is where Perplexity, Claude Cowork and Amoeba are
  all heading; Sikemux's ops plugins are its version of that.

---

## 13. Agent-to-agent as built (audit, 2026-10-15)

Read-only audit of the code on `main` against ADR 0012 and the rivals. Core paths below are under
`src-tauri/crates/sikemux-core/src/server/`; line numbers may drift.

**What exists.** `agent_list`, `agent_send`, `agent_spawn`, `agent_sessions`, `agent_read`, `agent_wait`,
`ask`, `board_add/list/claim/update` and `git_commit`, all answered in the core and gated on each agent's
own secret. Limits: two levels below an agent the person started, 6 open children per agent, 8
agent-started agents per project. Workers inherit the lead's account and permission mode; the lead can't
pick it. Report-back sends a worker's final answer at each turn end. `ask` goes to the lead first and
reaches the person if the lead doesn't answer. Board Auto works cards the person added: a worktree per
card, commit, push, PR on GitHub or Bitbucket, merge on CI or the project's check command, one rebase and
one fix, up to 3 rounds of review comments, Jira status kept in step. The rail, phone and notch show
teams under their lead, questions, and the board.

**Which harnesses take part.**

| Harness | Tools | Messages mid-turn | Its own "ask" routed |
|---|---|---|---|
| Claude (native) | yes | yes | yes (AskUserQuestion) |
| Codex (native) | yes | yes (`turn/steer`) | no: answered empty |
| Muse (native) | yes | yes | no |
| OpenCode, Hermes, Pi, Grok, OMP (ACP) | yes | only if the agent offers steering; otherwise queued to turn end | no |
| Terminal agents | yes | no; pasted in when idle | no |

A Claude lead can start a Codex, OpenCode or open-model worker today, and route Claude or Codex through a
connected provider. Mixed-vendor review works by hand; nothing builds it in.

**Verdict.** Complete on one host and well differentiated: teams mix vendors and models without
depending on any vendor's own team protocol, terminal agents join in, and the phone and notch answer
questions. Claude Code's agent teams are Claude-only; T3 Code is ahead only on spawning across machines.
The weak spots are trust, review and durability.

**Gaps against the ADR and rivals.**
- No spawn across hosts (T3 V2 `delegate_task`).
- No blind reviewer: any agent can `agent_read` any other, across projects, so a reviewer can't be kept
  from the builder's reasoning.
- No review gate before Auto merges, and no acceptance criteria on cards.
- No audit log of messages: they live only in each recipient's transcript and a 10-minute in-memory
  digest.
- Queued messages and open questions survive an app update but not a crash or plain restart; a blocked
  worker could wait forever.

**Problems found, most serious first.**
1. **An agent can act as the person.** Any process running as the same user can register as the window
   (`connection.rs`), then fetch any agent's secret or call person-only actions: answer questions, add
   board cards Auto treats as the person's, or change the board's check command, which is a shell
   command. Registering as the window also breaks the real window's pending calls. Fix: give the window a
   token agents can't reach, instead of trusting every local connection.
2. **The git guard misses bypass mode.** It runs only when an agent asks permission
   (`chat/native/session.rs`), so Codex, Muse and ACP agents in bypass mode, the default, and terminal
   agents can run `add -A`, `stash` or `reset --hard`. Only Claude's hook always runs.
3. **Auto's merge gate is partial.** It checks open questions only for its own agent at ship time
   (`board/finish.rs`); a worker's assumption, or a question raised after shipping, doesn't block the
   merge, though the ADR says it should.
4. **Messages can be lost.** Two identical worker reports within 2 minutes drop the second
   (`messages.rs`). A terminal paste always counts as delivered, even if the prompt check was wrong. Any
   message from a lead clears all of its worker's open questions, related or not.
5. **Codex's own questions vanish:** `requestUserInput` is answered empty instead of going through `ask`
   (`codex/state.rs`).
6. **The depth limit can be bypassed:** when a middle lead closes and its workers are kept, a
   grand-worker counts as one level down and can spawn again (`names/mod.rs`).
7. **Polling instead of events:** a 400 ms tick drives questions, board, Auto and delivery
   (`server/mod.rs`), and messages wait for the tick plus 1.5 s after a turn ends instead of reacting to
   it. A few small maps are never pruned. Two agents can keep replying to each other up to 30 times per
   10 minutes each way.

**Next steps, ranked.**
1. Close the identity boundary (problem 1).
2. A blind reviewer stage in Auto on a different vendor, acceptance criteria on cards, and open questions
   checked across the whole team before merging (problem 3).
3. Durable message queue and questions, plus a per-project message log the rail and phone can show.
4. Events instead of the tick, and fix the loss paths (problems 4 and 5).
5. Enforce the git guard for every driver and warn when an agent edits lines another live agent hasn't
   committed (problem 2); then spawn across hosts over iroh, with lineage.

**Status after the fixes (2026-10-15, on `main`, in no release yet).** The audit above is kept as found.
- Problem 1, fixed: the core checks the code signature of whatever connects to its socket; only Sikemux's
  own window and notch act as the person. A second registration no longer breaks the real window, only
  the app can swap in a new core binary, and the core tells which agent made a tool call from the process
  tree it started, so nothing secret sits in an agent's environment. Builds not signed by Sikemux's team
  trust their own app and notch by path. Still open: Linux checks the window and notch by path only.
- Problem 2, fixed: every agent Sikemux starts runs git through a guard that refuses sweeping commands in
  the shared checkout and allows them in its own worktree. Grok and Hermes rebuild PATH from the login
  profile, so a hook checks each of their shell commands by the same rule: Grok chats load it as a
  plugin, Grok in a terminal and Hermes read it from a private home the core gives every launch, whoever
  starts it.
- Problem 3, fixed: Auto checks the whole team at merge time, and merges only after a blind reviewer on
  another CLI, which sees only the card, its acceptance criteria and the diff, approves that exact
  commit. Not yet run end to end against a real pull request.
- Problems 4 and 5, fixed: reports are never dropped or refused, terminal pastes count only once taken, a
  lead's message closes only the question it answers, Codex's and Muse's questions reach the lead, and
  waiting messages and questions survive a crash. Secret questions go straight to the person and are
  masked on the Mac and phone.
- Problem 6, fixed, and the unpruned maps.
- Problem 7, fixed: events and per-item deadlines replace the tick for messages, questions, the board
  and Auto; looping pairs are held after a few rounds and named in the inbox. A per-project message log
  shows on the Mac board and the phone.
- Left: spawn across hosts with lineage, and collision warnings between agents in the shared checkout.

---

## 14. Go-to-market

- X: short screen recordings of what rivals can't show (two vendors' agents talking, an open model
  reviewing Claude's work, the inbox on the phone, closing the lid while the shed keeps going).
- Show HN for the Host/Linux launch, not before.
- Comparison pages: vs T3 Code, herdr, Orca, Conductor, cmux.
- Phone app in the App Store catches "Claude Code / Codex mobile" searches; word the listing carefully.
- Discord for retention.

## 15. Website implications

Once the next nightly ships, sikemux.com can claim the model layer (providers, coding plans, local
runners, tool-call check), the inbox, per-line agent attribution, and agent teams that mix vendors. It can
already claim browser annotate-to-agent and native chat for many harnesses. The Host and Team still need
a pricing page when they ship.

## 16. Main sources

- Posts: [owengretzinger](https://x.com/owengretzinger/status/2109846365065052351),
  [brunogsantos](https://x.com/brunogsantos/status/2109964126810517775),
  [fabienpenso](https://x.com/fabienpenso/status/2109881742089941348),
  [eliasstravik](https://x.com/eliasstravik/status/2109777235687202876),
  [rafalwilinski](https://x.com/rafalwilinski/status/2109940991868252302),
  [karpathy](https://x.com/karpathy/status/2109361546505966046),
  [jullerino](https://x.com/jullerino/status/2107976989810524309),
  [dhh](https://x.com/dhh/status/2106755814421565495)
- [Anthropic: Redesigning source control](https://www.anthropic.com/engineering/redesigning-source-control),
  [HN thread](https://news.ycombinator.com/item?id=50073633)
- [Anthropic: Managed Agents](https://anthropic.com/engineering/managed-agents),
  [Huntley: agency](https://ghuntley.com/agency/),
  [Semgrep: The End Is Nigh for Code Review](https://semgrep.dev/blog/2026/end-is-nigh-for-code-review)
- T3 Code: [repo](https://github.com/pingdotgg/t3code), [site](https://t3.codes),
  [remote internals](https://github.com/pingdotgg/t3code/blob/main/docs/internals/remote.md)
- herdr: [repo](https://github.com/herdrdev/herdr), [socket API](https://herdr.dev/docs/socket-api/),
  [plugins](https://herdr.dev/docs/plugins/), [seed](https://herdr.dev/blog/herdr-raised-a-seed/),
  [cloud](https://herdr.dev/cloud/)
- Herdr UI: [repo](https://github.com/penso/herdr-gpui), [site](https://herdrui.dev)
- [Perplexity: Building shared workspaces for people and agents](https://www.perplexity.ai/hub/blog/building-shared-workspaces-for-people-and-agents),
  [Aravind Srinivas](https://x.com/AravSrinivas/status/2109738344011972893)
- [Amoeba](https://useamoeba.com/), [pricing](https://useamoeba.com/pricing), [docs](https://useamoeba.com/docs)
- Open models: [Artificial Analysis open-weights launches](https://artificialanalysis.ai/articles/recent-open-weights-model-launches),
  [OpenRouter apps](https://openrouter.ai/apps), [OpenRouter Exacto](https://openrouter.ai/blog/announcements/auto-exacto/),
  [Kimi Vendor Verifier](https://kimi.com/blog/kimi-vendor-verifier), [z.ai Coding Plan](https://docs.z.ai/devpack/overview),
  [OpenCode Go](https://opencode.ai/docs/go/), [Synthetic](https://synthetic.new/pricing),
  [LM Studio with Claude Code](https://www.lmstudio.ai/docs/integrations/claude-code),
  [Ollama on MLX](https://registry.ollama.ai/blog/mlx),
  [JetBrains Air local models](https://blog.jetbrains.com/air/2026/07/what-s-new-air-gets-more-agents-local-models-and-java-kotlin-code-intelligence/),
  [Stack Overflow 2026 AI survey](https://survey.stackoverflow.co/2026/ai/data)
- Cloud: [Fly AI sandbox pricing](https://fly.io/learn/ai-sandbox-pricing/), [Sprites](https://fly.io/sprites),
  [Amp orb costs](https://ampcode.com/docs/orbs/sizes-and-costs), [Conductor pricing](https://www.conductor.build/pricing),
  [Factory Droid Computers](https://docs.factory.ai/droid-computers/overview), [exe.dev](https://exe.dev/),
  [Hetzner 2026 price rise](https://wz-it.com/en/blog/hetzner-price-increase-june-2026-cpx-ccx-alternatives/),
  [Codex auth](https://learn.chatgpt.com/docs/auth)
- [Claude Code legal and compliance](https://code.claude.com/docs/en/legal-and-compliance),
  [Agent SDK plan usage](https://support.claude.com/en/articles/15036540-use-the-claude-agent-sdk-with-your-claude-plan)
- [Superset pricing](https://superset.sh/pricing), [Termius pricing](https://termius.com/pricing),
  [vibe-kanban shutdown](https://vibekanban.com/blog/shutdown),
  [Mac coding agent apps comparison](https://rywalker.com/research/mac-coding-agent-apps)
- [Agent Trace](https://agent-trace.dev/), [git-ai](https://usegitai.com/docs/get-started/how-git-ai-works),
  [GitButler parallel agents at Trigger.dev](https://trigger.dev/blog/parallel-agents-gitbutler)
