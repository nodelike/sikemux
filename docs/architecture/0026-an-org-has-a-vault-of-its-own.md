# ADR 0026: An org has a vault of its own

- Status: Accepted
- Decision date: 2026-11-17
- Amends: [ADR 0016](./0016-the-account-vault.md)
- Amended: 2026-11-18, after a review against a hostile server: wraps name their sealer, a wrap
  is never overwritten, only an owner's or admin's host hands the key to another account's host,
  hosts refuse what is older than they hold, and a missing or untrusted answer no longer erases a
  host's copy

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
  member's host reads it. A shared project's rules are the one exception, below.
- **A project a member shares with the org has two entries: its store and its waiting list.**
  Each sits at an id the server works out from the project's hash and the kind
  (`project-memory`, `project-memory-waiting`), and the server takes a put there only while some
  member shares that project. Any member's host may put the waiting list, and the store too while
  the org's policy lets every member add rules; otherwise only an owner's or admin's host puts
  the store. The server removes both when the last member's share of the project ends.
- **A member's host writes only those two kinds, and never takes an entry out.** Its core sends no
  other kind, and the server refuses a member's put of any other kind and every member's removal.
- **A member's host gets the key from a host that already holds it.** The server lists the
  members' hosts that lack the key, and a host holding it wraps it for them: an owner's or
  admin's host for any of them, any other member's host only for its own account's. The org
  taking a person in is its admins letting their hosts in, so a new member's first computer
  waits until an owner's or admin's computer is on.
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
- **The server sees each entry's kind, size, revision and the host that last wrote it, and
  nothing else of it.** For a project's entries it also knows which shared project they are of,
  by hash. `model-key` is the first kind.
  An entry may carry the account vault's `access` list of hosts; a host the list leaves out does
  not use it.
- **A lapsed plan freezes the vault and keeps it.** While an org's Team plan is not paid, the
  server refuses new entries and the core hands none of the org's entries to agents. It deletes
  nothing, and replacing the key still works.
- **An org's model key is a provider of its own, never a stand-in for the person's.** On each
  member's host it shows among the connected providers as the catalogue provider marked with the
  org, under its own id, and agents start on it only when it is chosen. The person's own key to
  the same provider stays a separate choice, and members cannot change or disconnect it.
- **The org's audit log shows each change to the vault**: its making, each computer given the
  key, by its member's name and the computer's, each new key, and each entry written or removed,
  by kind.
- **The org-facing routes are staff-only until billing is live**, as the org routes are.

## What holds against a server that turns hostile

Someone who reads the server's data, or takes a copy of it, gets nothing of an org's vault: it
keeps only sealed entries and wrapped keys. Against a server that lies in what it answers:

- **A wrap names its sealer.** Each wrap carries the sealing host's public key, and its key is
  derived from that host's own key as well as a one-off one, so only that host could have made
  it. A host takes a wrap only from itself or from a host the vault lists as holding the key.
  The host that makes the vault, or replaces its key, keeps the key it made rather than reading
  it back, and a host holding the key at a generation ignores any other wrap at that generation.
- **A stored wrap stays, and members wrap only for their own hosts.** Adding wraps never
  replaces one a host already has, and the server takes a wrap for another account's host only
  from an owner's or admin's host, so a member's host can neither overwrite another's wrap nor
  write the first one. A wrap stays with its host key, so a host key that moves to another
  member's account before the next new key keeps the wrap its old account's member sealed.
- **Nothing older than a host holds now is taken in.** A host refuses a generation of the key
  older than the one it holds, an entry sealed under another generation than the vault's, and a
  revision older than the copy of that entry it holds, and keeps what it held. For each entry it
  saw go, whether the server listed it as removed or a full read left it out, it keeps a mark at
  least one revision above the last it held, or the server's own if higher, and refuses the entry
  at or below it, dropping a held copy that is not above the mark. It does not replace the key
  while the server lists an entry it refuses. The server can still keep a removal from a host, or
  understate its revision to a host that missed writes before it, and then hand it a write the
  removal made it refuse, as a later revision of the entry. A host that neither held an entry nor
  was told it went has no mark for it, so the server can list it again. A host replacing the key
  seals again what the server lists, so an entry taken back this way crosses into the new
  generation as a valid entry of it.
- **A failed answer at the generation a host holds does not erase its copy.** A missing or
  untrusted wrap at that generation, or the server saying the vault does not exist while the host
  holds its key, leaves the copy as it is and shows the failure. A host lets go only when the
  server names a higher generation it cannot open, as below, or says the account is not in the
  org, which is how a removed member is told.

What still rests on the server:

- **It decides who is a member, which hosts they have, and in which role.** Only its role check
  keeps a member's host from writing what an admin's may. It can list a host key of its own
  among the members' hosts, and the holders would wrap the key for it, since nothing lets them
  tell a member's real host apart. Once that key is listed as holding, the server can hand any
  host a key of its own, a host that holds the real one included: it names its key among the
  holders, raises the generation and wraps a new key from it, and that answer replaces the
  host's copy. The lie outlasts itself: the host then refuses the honest server's lower
  generation, keeps any entry the server forged at a high revision and hands it to agents, and
  an admin's host writes under a key no other host has. Only signing out clears it. This is the
  gap ADR 0016 records for the account vault, and the same step would close it.
- **A host that holds nothing takes what the server gives.** A host that is waiting has only its
  generation to compare against. One answer with a higher generation and no wrap for the host
  empties the copy it keeps, and pins that generation until it signs out; a 404 or a sign-out
  empties it too. So the server can roll a host back in two answers: one that empties it, then
  an old copy.
- **Restoring the server from an older backup** strands the hosts that held newer. Each keeps
  the key generation and the entry copies it held and refuses the backup's older ones, and the
  server refuses admins' writes to those entries, since they name revisions it no longer has.
  Entries the backup lacks, hosts take for removed. Bringing the server's data forward again
  moves them on, and so does signing a host out and in, which takes the backup's state.
- **A wrap waits on its sealer.** A wrap whose sealer has left the org lists no holder that hosts
  trust, so its host cannot open it until the rotation that leaving asks for. If no host holds
  the key any more, what it held is lost: there is no recovery key. An owner's or admin's host
  can then make the vault again. In one step the server drops every entry and wrap, raises the
  generation and keeps only the asker's wrap of the new key, writing how many it dropped to the
  org's log. The server refuses a host the vault already opens on, and both the core and the
  server refuse while another computer that would seal the key
  for it, its own account's or an owner's or admin's, may be online, and name that computer. The
  server counts one as online while it is connected, was seen in the last two minutes, or the
  server started too recently to know, since a restart forgets who was connected. Only an owner's
  or admin's host whose own wrap does not open is told which computers hold the key and when each
  was last online. The server alone gains nothing from a remake it could not already do by
  emptying the vault: hosts still take a key only from a listed holder.
- **A remake is told from a rotation only by the server's `remadeAt`.** A host let in at the new
  generation within one read has no evidence of its own to tell the two apart, so it takes the
  server's word. #915 is for evidence the host holds itself.
- **A host that cannot open the current generation holds nothing.** It lets go of the key and
  every entry, mid-rotation too, until a holder seals the new key for it, keeping only the
  generation, below which it refuses anything. It never writes back what it held: of a
  shared project's rules and knowledge, its own person's go back to the project's store here,
  the rest is forgotten, and none of it enters the new vault without that person's tap.

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
