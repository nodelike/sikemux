# ADR 0016: The account vault

- Status: Accepted
- Decision date: 2026-10-16

## Context

Agents run on hosts unattended and the person joins from any device
([ADR 0014](./0014-agents-run-on-hosts-and-every-client-is-a-way-in.md)). The keys and sign-ins
they need are kept per host today: model providers' keys in the Keychain on a Mac and in private
files elsewhere, and each plugin's sign-in on the host where it was made. A new host starts with
none of them, and the account's browser profile
([ADR 0015](./0015-one-browser-per-account-seen-from-every-device.md)) needs somewhere to live
when its home goes away.

## Decision

- **One vault per account holds what every host needs.** Model providers' keys, plugin sign-ins
  such as GitHub, Jira, Bitbucket, Slack and AWS, database logins, and the saved browser profile.
- **The accounts server cannot read it.** Everything in the vault is encrypted under a vault key
  the server never sees, as shared chats are sealed
  ([ADR 0011](./0011-shared-chats-are-sealed-and-relayed.md)). The vault key is wrapped once for
  each of the account's hosts, with the key each already has from joining. Devices never hold it:
  they ask a host to act. The server keeps only the wrapped copies and the encrypted entries.
- **A new host gets the vault when it is allowed in.** When a device that acts as the person
  approves a host joining the account, a host that already holds the vault key wraps it for the
  new one. Removing a host from the account makes a new vault key and wraps it for those left.
- **Each entry says who may use it.** Which hosts, and which agents or projects. Nothing is
  allowed by default.
- **Agents never hold a secret.** The core makes plugin calls itself, as it does now, and adds
  tokens to a sandboxed agent's traffic on its way out, so an agent can use a service but never
  read or leak its token. A secret the person types goes through a masked question that never
  reaches a transcript.
- **Each host keeps a local copy in its own secret store.** The Keychain on a Mac, private files
  elsewhere, as `sikemux-keychain` does today, refreshed when an entry changes.
- **Agent sign-ins stay out of it.** An agent CLI's own login, such as Claude Code's or Codex's,
  is made on each host through the vendor's own flow, relayed to whichever device the person
  holds, and never copied: vendors forbid passing those credentials on.

## Consequences

- A host joined to the account can use the person's keys and plugin sign-ins at once, within what
  each entry allows, with nothing to set up again.
- Losing every host that holds the vault key loses the vault. Recovery needs a key the
  person keeps, shown once when the vault is made.
- A stolen device can ask a host to act with the vault's entries but never reads them, since it
  never holds the key; revoking it from any other device or the web app cuts it off.
- Every use of an entry is logged with the agent or device, the host and the time, and shows in
  the inbox and the web app.

## Alternatives considered

- **Keeping secrets per host, set up again on each.** Rejected: every new host would need the
  person at it before agents could work.
- **The accounts server holding secrets it can read.** Rejected: one breach of the server would
  expose every account's keys and sign-ins.
- **Copying agent CLI sign-ins between hosts.** Rejected: vendors forbid it, and a copied
  subscription login can be revoked for every host at once.
