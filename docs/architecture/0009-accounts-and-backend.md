# ADR 0009: Accounts let a phone find and join its owner's Macs

- Status: Accepted
- Decision date: 2026-10-02
- Superseded in part: 2026-10-21, by [ADR 0018](./0018-the-account-is-the-trust.md): a valid join
  ticket from the host's own account lets a device in without asking, unless the host is locked

## Context

A phone pairs with a Mac by scanning a QR code or typing a six-digit code shown on that
Mac ([ADR 0007](./0007-remote-access-over-iroh.md)). That needs the person at the Mac, and
it has to be repeated for every Mac and every phone. People should open the phone app
anywhere and see their own Macs, join one with a tap, and remove a lost phone from every
Mac at once.

That needs something both devices can reach that knows who owns them: accounts and a
server. ADR 0007 rejected accounts as the root of trust, because a stolen account must
not become a remote shell on someone's Mac. This decision keeps that rule.

## Decision

- **Accounts are run by Clerk.** People sign in with Google, GitHub, or email and a
  password. Clerk merges sign-ins that share a verified email, so there is no auth or
  account-linking code of our own.
- **Who needs an account.** The phone app does. The Mac app works without one and asks
  for one only when pairing a device. The Mac signs in through the default browser
  (OAuth with PKCE, Clerk as the provider), never through a form inside the app, and
  keeps its token in the Keychain.
- **Devices prove their keys.** Each device registers the iroh key it already has by
  signing a one-time challenge with it. A Mac's core registers as a host and a phone as
  a client. A phone lists only hosts; it never sees other phones.
- **Joining without a code.** For a Mac on the same account, the server gives the phone
  a ten-minute join ticket, signed by the server, naming the phone's key and the account.
  The phone presents it to the core over a new channel beside code pairing. The core
  checks the ticket itself, without calling the server: the signature, the expiry, that
  the account is its own owner's, and that the phone holding the ticket is the one named
  in it. Then the Mac asks, as code pairing does, whether to allow the phone and with
  full control or watch only. From then on the phone is an ordinary paired device and
  the server takes no part in its sessions.
- **The Mac's approval stays the last word.** A stolen account or a compromised server
  can ask a Mac to let a phone in, but cannot let it in. Code pairing remains for Macs
  that are not signed in.
- **Removing a device revokes it everywhere.** Removing a phone from the account, in the
  phone app or the web app, revokes it on every Mac of that account and drops its open
  connections.
- **Changes are an event log; WebSockets only signal.** Every change writes an event to
  Postgres in the same transaction as the change. Each device keeps one WebSocket to the
  server, which pushes new events. Each device acknowledges the last event it handled,
  and on reconnect receives everything after it, so a Mac that was offline still revokes
  a phone when it comes back. Delivery is at least once, so every handler tolerates
  repeats. A core authenticates its socket by signing a challenge with its key; phones
  and the web app use their Clerk session.
- **One schema is the contract.** A JSON Schema in `server/protocol/` describes the HTTP
  API and the live messages. TypeScript types, the Rust types the core uses and the
  OpenAPI document are generated from it, and CI fails if they drift. Routes are
  versioned (`/v1`); within a version they only grow.
- **One server, one database.** The backend is TypeScript (Hono on Node) in `server/`, a
  pnpm workspace of its own like `mobile/`. It is one process organised by feature, with
  Postgres for the data, the event log, signals between processes (LISTEN/NOTIFY) and,
  later, background jobs. The web app at app.sikemux.com lives in `server/app/` and
  deploys with the API at api.sikemux.com. Merging a change under `server/` to `main`
  deploys it, with an automatic rollback when the health check fails.
- **Only identity reaches the server.** It stores accounts, device keys and names, and
  the event log. Chats, terminals, prompts and code still travel only between the phone
  and the Mac over iroh.

## Consequences

- Paired phones keep working while the server is down. Signing in, joining a new Mac and
  the web app's live view stop until it returns, and revocations arrive when it does.
- The core gains a channel for join tickets, a signing request only the local app can
  make, a WebSocket client, and two new values in `remote.json`: its owner and the
  server's ticket key.
- The phone app gains a sign-in screen, and `sikemux-mobile` gains signing and joining.
- Push notifications can be added later as an event and a background job, without new
  infrastructure.
- We now run a server: backups, a monthly restore test, monitoring and alerts are part
  of the work.

## Notes

- 2026-10-03: Phones prove their device key on the live connection as well as showing their
  Clerk session, the same signed challenge a Mac's core answers. That ties each socket and
  its cursor to one phone, so another session on the account cannot acknowledge events for
  it. Removing a phone from elsewhere also revokes the Clerk session that registered it, and
  deleting an account deletes the user in Clerk; deleted accounts are erased 30 days later.

- 2026-10-14: A host with no browser, such as a Linux box reached over SSH, signs in by code,
  as in OAuth's device grant. `sikemux login` asks the API for a secret code and a short
  user code, shows the user code and a link, and asks every five seconds for at most ten
  minutes. The person opens the link on app.sikemux.com, or enters the code in the phone app,
  checks the host's name and approves it. The host then gets the account and a fresh challenge,
  signs it as any device registers, and is on the account. It keeps no sign-in token: its core
  already proves itself with its key on the live connection. The API stores only a hash of the
  secret code. Clerk offers no device grant; RFC 8628's own polling is used because an
  unregistered host has no live connection to be told on, and it asks only while someone is
  signing it in. The user code is eight of twenty consonants, good for ten minutes, and
  lookups are limited per account, so guessing one is out of reach.
- 2026-10-14: A host without a window still asks before letting each device in, but someone
  other than the person at the host may answer: a device already paired with full control that
  joined through the host's own account. The host remembers which account each device's
  ticket named and checks it against its owner on every request, so a stolen account or a
  compromised server still cannot let a device in, and a host signed in to another account
  stops taking answers from the old one's phones. Such a device reaches the pairing requests
  and the paired devices through the core's `pairing`, `answerPairing` and `changeDevice`
  calls. The first device on a host with no window is answered in its terminal by
  `sikemux login` or `sikemux pair`.

## Alternatives considered

- **Bonjour on the local network only.** Needs no server, but finds a Mac only on the same
  network, and cannot revoke a phone on a Mac that is offline.
- **Accounts as the root of trust.** Still rejected, as in ADR 0007: the server suggests,
  the Mac decides.
- **Our own auth, or Rust or Go for the server.** Account merging, OAuth providers and
  email verification are a product of their own. TypeScript shares types with the phone
  and web apps.
- **Server-sent events, Redis pub/sub or a message queue for live updates.** Server-sent
  events only go one way and we will want messages from devices too. Redis and queues add
  a system to run, while Postgres already guarantees the order and survives restarts.
  The publishing and job code sits behind small interfaces in case we outgrow Postgres.
- **A managed database.** Postgres on our own server, with nightly off-site backups, is
  enough for now.
- **Kubernetes, microservices, Kafka or GraphQL.** Far more to run and understand than a
  directory of devices needs.
