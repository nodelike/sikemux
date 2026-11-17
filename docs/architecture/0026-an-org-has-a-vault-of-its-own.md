# ADR 0026: An org has a vault of its own

- Status: Accepted
- Decision date: 2026-11-17
- Amends: [ADR 0016](./0016-the-account-vault.md)
- Amended: 2026-11-18, after a review against a hostile server: wraps name their sealer, a wrap
  is never overwritten, hosts refuse what is older than they hold, and one bad answer no longer
  erases a host's copy

## Context

An org's owners and admins want to hand their members what the org pays for, starting with a
model provider's key, without each member pasting it in on each computer. The account vault
(ADR 0016) holds one person's secrets for that person's hosts. An org's secrets are a different
group's and must reach a different set of hosts: every host of every member, and no others.

## Decision

- **Each org has its own vault, apart from every member's account vault.** It has its own key,
  which the accounts server never sees, and the server keeps only sealed entries and that key
  wrapped for each member's host. Nothing of a person's vault goes in an org's, or the other way.
- **The first write by an owner or admin makes it.** Their host makes a key, wraps it for itself
  and sends the wrap. Only an owner's or admin's host may make the vault, add or replace an
  entry, take one out, or replace the key; the server refuses any other member's host. Any
  member's host reads it.
- **A member's host gets the key from a host that already holds it.** The server lists the
  members' hosts that lack the key; any host holding it wraps it for each, as the account vault
  does for the account's hosts. The org taking a person in is its admins letting their hosts in.
- **When a member or a member's host goes, the key is replaced.** The server keeps the wraps of
  hosts that are no longer a member's and says the vault needs a new key. The next owner's or
  admin's host to read it makes a new key, wraps it for the members' hosts that are left, and seals
  every entry again under it in one request; the server drops every other wrap. One request
  wraps the new key for at most 500 hosts, its own first, then those that held the old key; the
  rest wait for the next round. A host whose
  account left the org, whose account let it go, or that signed out drops its copy of every org
  vault.
- **Each host keeps its copy in its own secret store**, in one item apart from the account
  vault's, holding each org's key and opened entries, so a host that cannot reach the server
  still has them.
- **An entry is bound to its place.** Each is sealed with the org, the entry's id and kind, the
  key's generation and its revision, so the server cannot move an entry to another org, change
  its kind, or pass one generation's copy off as another's. Wraps are bound to the org, the
  generation and the host, apart from the account vault's.
- **The server sees each entry's kind and nothing else of it.** `model-key` is the first kind.
  An entry may carry the account vault's `access` list of hosts; a host the list leaves out does
  not use it.
- **A lapsed plan freezes the vault and keeps it.** While an org's Team plan is not paid, the
  server refuses new entries and the core hands none of the org's entries to agents. It deletes
  nothing, and replacing the key still works.
- **An org's model key is a provider of its own, never a stand-in for the person's.** On each
  member's host it shows among the connected providers as the catalogue provider marked with the
  org, under its own id, and agents start on it only when it is chosen. The person's own key to
  the same provider stays a separate choice, and members cannot change or disconnect it.
- **The org's audit log shows each change to the vault**: its making, each host given the key,
  each new key, and each entry written or removed, by kind.
- **The org-facing routes are staff-only until billing is live**, as the org routes are.

## What holds against a server that turns hostile

Someone who reads the server's data, or takes a copy of it, gets nothing of an org's vault: it
keeps only sealed entries and wrapped keys. Against a server that lies in what it answers:

- **A wrap names its sealer.** Each wrap carries the sealing host's public key, and its key is
  derived from that host's own key as well as a one-off one, so only that host could have made
  it. A host takes a wrap only from itself or from a host the vault lists as holding the key.
  The host that makes the vault, or replaces its key, keeps the key it made rather than reading
  it back, and a host holding the key at a generation ignores any other wrap at that generation.
- **A stored wrap stays.** Adding wraps never replaces one a host already has, so one member's
  host cannot overwrite another's and lock it out.
- **Nothing older than a host holds is taken in.** Each host keeps the newest generation it took
  and each entry's newest revision, removed entries included. It refuses an older generation, an
  entry sealed under another generation than the vault's, and an older revision, and keeps what
  it held; it does not replace the key while the server lists such an entry. The server can still hold an entry back, which hosts take for its removal.
- **One answer does not erase a host's copy.** A missing or untrusted wrap, or the server saying
  the vault does not exist while the host holds its key, leaves the copy as it is and shows the
  failure. Only the server saying the account is not in the org, which is how a removed member
  is told, makes a host let go.
- **The server alone decides who is a member, and in which role.** Only its role check keeps a
  member's host from writing what an admin's may. It can also list a host key of its own among
  the members' hosts, and the holders would wrap the key for it, since nothing lets them tell a
  member's real host apart; once listed as holding the key, such a host could seal a key of its
  own for a host that holds none yet. This is the gap ADR 0016 records for the account vault,
  and the same step would close it.

There is no recovery key: an org whose every holding host is gone makes its vault again.

## Consequences

- An admin adds a provider key once and every member's host can start agents on it.
- Removing a member, or a member's host, costs one more round by an admin's host before the
  removed host's key no longer opens new entries. The entries it could already read it read.
- An org's GitHub App key and its skills, subagents and MCP servers can be further kinds of
  entry in the same vault.

## Alternatives considered

- **Keeping org secrets in each admin's account vault and copying them to members.** Rejected:
  it mixes the person's secrets with the org's, and a member's host would need the admin's key.
- **Letting the server hold the org's key.** Rejected for the reason ADR 0016 rejects it.
- **Using the org's key as the member's own key to that provider.** Rejected: nothing should pick
  the org's key, or the person's, without the person choosing it.
