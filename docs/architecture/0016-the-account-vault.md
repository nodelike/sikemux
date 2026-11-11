# ADR 0016: The account vault

- Status: Accepted
- Decision date: 2026-10-16
- Amended: 2026-10-19, to say which credentials are shared and how a per-host agent login stays
  within the vendors' terms, and that agents from a linked host use none of them until allowed;
  2026-11-11, to say how sign-ins a host already has move in, and what the server sees, how a
  host gets the key and how a shared sign-in is refreshed

## Context

Agents run on hosts unattended and the person joins from any device
([ADR 0014](./0014-agents-run-on-hosts-and-every-client-is-a-way-in.md)). The keys and sign-ins
they need are kept per host today: model providers' keys in the Keychain on a Mac and in private
files elsewhere, and each plugin's sign-in on the host where it was made. A new host starts with
none of them, and the account's browser profile
([ADR 0015](./0015-one-browser-per-account-seen-from-every-device.md)) needs somewhere to live
when its home goes away.

## Decision

- **One vault per account holds what every host needs.** Model providers' API keys, plugin
  sign-ins such as GitHub, Jira, Bitbucket, Slack and AWS, database logins, and the saved browser
  profile. These are the person's own to place on their machines: a provider key is billed to its
  owner, which Anthropic's terms allow in "a secrets manager… for use by the customer's own
  authorized users", and a plugin sign-in was issued to Sikemux's own OAuth app for this person.
  Agents that run on a provider key, such as OpenCode, OMP, Hermes and Pi, take it from here.
- **The accounts server cannot read it.** Everything in the vault is encrypted under a vault key
  the server never sees, as shared chats are sealed
  ([ADR 0011](./0011-shared-chats-are-sealed-and-relayed.md)). The vault key is wrapped once for
  each of the account's hosts, with the key each already has from joining. Devices never hold it:
  they ask a host to act. The server keeps only the wrapped copies and the encrypted entries.
- **A new host gets the vault when it is allowed in.** When a device that acts as the person
  approves a host joining the account, a host that already holds the vault key wraps it for the
  new one. Removing a host from the account makes a new vault key and wraps it for those left.
- **Each entry says who may use it.** Which hosts, and which agents or projects. Nothing is
  allowed by default. An agent another host's agent started or messaged uses none of them on
  this host until the person allows that host
  ([ADR 0013](./0013-hosts-link-to-run-agents-for-each-other.md)).
- **Agents never hold a secret.** The core makes plugin calls itself, as it does now, and adds
  tokens to a sandboxed agent's traffic on its way out, so an agent can use a service but never
  read or leak its token. A secret the person types goes through a masked question that never
  reaches a transcript.
- **Each host keeps a local copy in its own secret store.** The Keychain on a Mac, private files
  elsewhere, as `sikemux-keychain` does today, refreshed when an entry changes.
- **One place refreshes each sign-in.** Some services hand out a new refresh token each time and
  void the old one, as Atlassian does, so two hosts refreshing the same sign-in lock each other
  out. The host that made the sign-in refreshes it and writes the new tokens back to the vault;
  the others only read. When that host leaves, another takes it over.
- **Hosts the person does not control get keys, not subscriptions.** A hosted sandbox can use
  the person's provider keys, billed to them. Subscription logins stay on the person's own
  machines ([ADR 0014](./0014-agents-run-on-hosts-and-every-client-is-a-way-in.md)).
- **Agent sign-ins stay out of it, and are made once per host.** An agent CLI's subscription
  login, such as Claude Code's, Codex's with a ChatGPT plan, Grok's or Gemini's, is never stored,
  copied or handed between hosts. Anthropic's terms say developers "may not collect, store, or
  intermediate Claude.ai credentials or session tokens — sign-in to a Claude account must complete
  through Anthropic's own flow", and the other vendors' logins are treated the same way. When a
  host needs one, Sikemux runs that host's own login, such as `claude /login` or
  `codex login --device-auth`, in a terminal shown on the device the person holds. The person
  finishes it on the vendor's page and types any code into the agent's own prompt; Sikemux carries
  the keystrokes as a terminal does and never reads or keeps the result. A long-lived token from
  `claude setup-token` is the person's to set on a machine themselves; Sikemux does not hold it.
- **Sign-ins a host already has move in when it joins.** The host that makes the vault, and each
  host that joins later, puts its connector sign-ins and database logins into the vault once.
  Each stays allowed for every agent on the host it came from, as before, and that host refreshes
  it; other hosts use it only once the person allows them. When two hosts hold the same identity
  on the same service, the newest token is kept and its host refreshes it; different identities
  become separate entries. AWS profiles stay in the AWS CLI's own files on each host.
- **A host with no account keeps its sign-ins in its own secret store**, as before the vault.
- **The server sees which plugin each entry is for, and nothing else of it.** An entry's label,
  such as the account it signs in as, is sealed with its tokens, its plugin settings and who may
  use it. The server keeps each entry's plugin, revision and the host that last wrote it, and the
  use log keeps the entry, its plugin, the host, the agent and the time, so the web app can show
  "GitHub, used by @Otter on mini" without the key.
- **A host signed in to the account gets the key from any host that holds it.** Signing in is
  the person letting it in ([ADR 0018](./0018-the-account-is-the-trust.md)); it still uses no
  entry until they allow it. The vault key is wrapped with each host's Ed25519 key taken as its
  X25519 key, and the recovery key is an X25519 key the hosts know only the public half of, so a
  host that replaces the vault key wraps the new one for it as well.
- **The host that refreshes a shared sign-in renews it ahead of time.** It seals each new access
  token into the entry before the old one runs out, and the other hosts use that token rather than
  refreshing. Whichever host writes a newer token for the same identity becomes the one that
  refreshes it. An entry allowed only for some projects is matched by the project folder's name,
  which each host's copy of the project shares.

## Consequences

- A host joined to the account can use the person's keys and plugin sign-ins at once, within what
  each entry allows, with nothing to set up again.
- Losing every host that holds the vault key loses the vault. Recovery needs a key the
  person keeps, shown once when the vault is made.
- A stolen device can ask a host to act with the vault's entries but never reads them, since it
  never holds the key; revoking it from any other device or the web app cuts it off.
- Every use of an entry is logged with the agent or device, the host and the time, and shows in
  the inbox and the web app.
- Each new host asks the person once per subscription agent it runs. Every host signed in to the
  same plan draws on the same quota, which Anthropic sizes for "ordinary, individual usage", so
  many hosts running agents all day should use provider keys instead.

## Alternatives considered

- **Keeping secrets per host, set up again on each.** Rejected: every new host would need the
  person at it before agents could work.
- **The accounts server holding secrets it can read.** Rejected: one breach of the server would
  expose every account's keys and sign-ins.
- **Copying agent CLI sign-ins between hosts.** Rejected: vendors forbid it, and a copied
  subscription login can be revoked for every host at once.
- **Keeping a `claude setup-token` token in the vault and setting it on every host.** Rejected:
  it is a session token Sikemux would store and pass on, which Anthropic's terms forbid. The
  person may still set one on their own machines.
- **Every host refreshing a shared sign-in itself.** Rejected: services that rotate refresh
  tokens void every copy but the last.
