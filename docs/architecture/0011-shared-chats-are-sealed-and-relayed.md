# ADR 0011: Shared chats are sealed by the host and relayed by the accounts server

- Status: Accepted
- Decision date: 2026-10-13

## Context

People want to show a chat as it happens to someone else: a teammate on Sikemux, a
phone, or someone with only a browser. Phones already reach a host over iroh
([ADR 0007](./0007-remote-access-over-iroh.md)), but that path trusts one account's
devices, gives a device the whole host, and lets even a Watch device answer permission
requests. A browser cannot join it at all.

## Decision

- **A share is one chat, read-only, for anyone signed in to Sikemux who has its link.**
  It ends when its owner stops it or after its lifetime, 24 hours unless chosen otherwise
  and never more than seven days.
- **Viewers never reach the core.** The core attaches to the chat as a phone would, from
  inside, and pushes what it hears to the accounts server over `/v1/share`. The server
  passes it to viewers. A viewer can only ask, sealed, for an older page of the chat or a
  picture pasted into it; the core decides what to answer and refuses anything else.
- **The server cannot read a share.** The core seals every frame with AES-256-GCM under a
  key that only the link holds, after its `#`, which browsers never send to a server. The
  server sees share ids, sizes, times and who watches.
- **The server keeps frames only in memory**: the latest snapshot and what followed it,
  for late viewers. After a restart the core sends a fresh snapshot when it reconnects.
- **The share protocol is generated into `sikemux-core`, not `sikemux-wire`**, so it never
  changes the phone's native code ([ADR 0010](./0010-phone-links-only-the-wire-and-client.md)).
  The phone, the Mac and the web app read shares in TypeScript.
- **Every viewer draws the Mac's own transcript**: the Mac and the web app with the same
  components, the phone with its own chat screen, fed by the same events.

## Consequences

- A share costs the host one upload per event, whatever the number of viewers.
- Anyone the link is forwarded to can read the chat once signed in, and what an agent
  read, such as a file holding a secret, is in the transcript. The share dialog says so.
- A share outlives a core restart: the core keeps its shares, keys included, in its
  private `remote.json`.
- The relay holds plaintext metadata only, so the accounts server stays identity and
  transport, as [ADR 0009](./0009-accounts-and-backend.md) intends.

## Alternatives considered

- **Viewers joining the host over iroh, like phones.** Rejected: browsers cannot, every
  viewer would cost the host an upload, and it needs access tiers below Watch.
- **The server storing transcripts, as Amoeba's does.** Rejected: the server would hold
  what people's agents read.
