# ADR 0010: The phone links only the wire format and the client

- Status: Accepted
- Decision date: 2026-10-10

## Context

An over-the-air update reaches only phone builds with the same native code, and the
phone's runtime version is a fingerprint of that code, Rust included
([ADR 0008](./0008-mobile-app-expo-and-rust-client.md)). `sikemux-mobile` depended on the
whole of `sikemux-core`, which pulled in `sikemux-pty` and `workspace-hack`. Every change
to the core's server, every terminal change and every desktop dependency change gave the
phone a new fingerprint, so almost every desktop commit cut installed phones off from
updates until a store build went out. In the 60 days before this decision, 112 commits
touched `sikemux-core`; about two thirds of the files they changed were server code a
phone never runs.

## Decision

- **Two crates hold everything the phone links from the core.** `sikemux-wire` is what the
  core and its clients send each other: the protocol, the accounts types generated from
  `server/protocol/schema`, and the terminal and CLI shapes those carry. `sikemux-client`
  is how a client talks to a core: the connection and its requests, reaching a paired core
  over iroh, the phone's side of joining a host, and what a device signs for the accounts
  server.
- **`sikemux-core` builds on them.** It re-exports both under its old module names and
  adds what only the desktop does: finding, starting and upgrading the core on this
  machine, and the host's checks of a join ticket. `sikemux-pty` takes its wire types from
  `sikemux-wire`.
- **Neither crate joins `workspace-hack`.** hakari leaves them out, so the desktop app's
  dependencies never reach the phone's lockfile entries.
- **The phone's Rust is transport.** What changes with features, such as how a chat reads
  or how long a turn took, lives in TypeScript the phone shares with the Mac, and reaches
  the phone over the air.

## Consequences

- Changes to the core's server, its chat drivers, the harness, the terminal engine and the
  desktop's dependencies no longer change the phone's native code. Only a change to the
  wire format, the client, `sikemux-mobile` or a dependency they share needs a store build.
- A field the core adds inside JSON the phone already passes through, such as a chat
  update, reaches the phone with an over-the-air update. A field on a typed UniFFI record
  in `sikemux-mobile` still needs a store build.
- The moves themselves changed the fingerprint once, so the release that carries them
  goes out as a store build.
- `mobile/app/test/fingerprint.test.ts` pins the crates the phone counts, so a new
  dependency from either crate on the core shows up as a failing test.

## Alternatives considered

- **Putting the server behind a Cargo feature.** Rejected: the fingerprint hashes a
  crate's whole folder, so the phone's runtime would still change with every server edit.
- **Counting only the files the phone compiles.** Rejected: knowing them needs a build,
  and the fingerprint is worked out without one, on a Linux runner.
