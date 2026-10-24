# ADR 0010: The phone links only the wire format and the client, and the core owns what the data means

- Status: Accepted
- Decision date: 2026-10-10, revised 2026-10-21

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
- **A request the phone makes is a method of the core, called by name.** The wire format
  has one `Call { method, params }` request answered with JSON. The methods live in the
  core's server (`sikemux-core/src/server/calls.rs`) and the phone's TypeScript calls them
  through the bridge's `call` and `call_to_file`, so a new request reaches phones over the
  air. A method a released app calls is never removed or renamed, as the accounts API's
  `/v1` only grows. `call_to_file` writes a file in the answer to disk from Rust, so large
  answers never cross into JavaScript.
- **The phone's Rust is transport.** Neither crate does anything with the data it carries.
- **The core owns what the data means; clients own only how it is shown.** The core reads
  every agent's own messages and sends what a client draws: a chat folded into rows with
  stable ids (its turns, folds, timings, tool calls read once, letters from other agents,
  subagents), each agent's status and roll-ups, questions and where they stand, and the
  agent catalog. It sends changes, not snapshots: rows appended, text written on, fields
  set, rows taken back, a window of the latest rows when a client opens a chat, earlier
  pages on demand, and what a client missed when it comes back with a mark. Clients keep
  one store of that view, apply the changes and draw it, with only their own words, icons,
  formatting, path display and markdown drawing. No client reads an agent's messages, folds
  events or keeps a second copy of the core's logic, so the Mac, the phone, the notch, the
  web and a GPUI desktop all show the same thing from the same data.

## Consequences

- Changes to the core's server, its chat drivers, the harness, the terminal engine and the
  desktop's dependencies no longer change the phone's native code. Only a change to the
  wire format, the client, `sikemux-mobile` or a dependency they share needs a store build.
- What the phone shows of a host crosses the bridge as the core's own JSON: a chat's view
  and its changes, the device view and the host's info, drawn by TypeScript in
  `mobile/app/src`. A field the core adds there reaches the phone with an over-the-air
  update, and so does a change to what the data means, since the core works it out. Only
  what the native side acts on itself, such as a chat's place in its feed, stays a typed
  UniFFI record, and a field on one of those still needs a store build.
- A feature that changes what a chat or an agent means changes the core, once, and every
  client follows. One that changes how it looks changes each client that shows it.
- A new request type in `sikemux-wire`, or a new bridge method, still needs a store build,
  so neither is added for a feature: it becomes a method of `Call`. What still needs one is
  something new the core pushes outside the chat events and the device view, a transport
  change, or a native module the app adds.
- The moves themselves changed the fingerprint once, so the release that carries them
  goes out as a store build.
- `mobile/app/test/fingerprint.test.ts` pins the crates the phone counts, so a new
  dependency from either crate on the core shows up as a failing test.

## Alternatives considered

- **Feature logic in TypeScript the phone shares with the Mac.** The first form of this
  decision. Rejected on 2026-10-21: only clients that run that TypeScript could use it,
  the notch and a GPUI desktop and web cannot, and each client folded the same events
  again, holding a whole chat's raw events to do it.

- **Putting the server behind a Cargo feature.** Rejected: the fingerprint hashes a
  crate's whole folder, so the phone's runtime would still change with every server edit.
- **Counting only the files the phone compiles.** Rejected: knowing them needs a build,
  and the fingerprint is worked out without one, on a Linux runner.
