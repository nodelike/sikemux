# ADR 0008: The phone app is Expo, calling the core's own Rust client

- Status: Accepted
- Decision date: 2026-10-02

## Context

The phone app pairs with a Mac and drives its core over iroh ([ADR 0007](./0007-remote-access-over-iroh.md)).
The connection, the pairing exchange and the wire format already exist in Rust, in
`sikemux-core`, and are tested against the core. iroh has no JavaScript implementation
that runs in React Native, and a second client written in TypeScript would drift from the
core every time the protocol changes.

## Decision

- **Expo, with development builds.** Expo is the framework React Native recommends. A
  development build runs any native module, and `expo prebuild` still produces real
  `ios/` and `android/` projects, so a bare React Native project would add nothing. Expo
  brings EAS builds for TestFlight and the Play Store, push notifications, the camera for
  pairing QR codes, the secure store for the device key, routing and over-the-air updates.
  Expo Go cannot load our module, so it is not used.
- **The core's client, compiled for the phone.** `src-tauri/crates/sikemux-mobile` exposes
  `sikemux-core`'s client, pairing and iroh connection through UniFFI.
  `uniffi-bindgen-react-native` turns it into a Turbo Module (`@sikemux/native`), the way
  Mozilla's tooling and apps such as Fressh do. Requests and answers cross as the core's
  protocol JSON; terminal bytes cross as bytes.
- **One repository.** The app lives in `mobile/` beside the desktop app, because it
  compiles the same protocol crate: a protocol change lands in one commit for the Mac and
  the phone, and CI builds the mobile crate on every change. `mobile/` is its own pnpm
  workspace, so the desktop install, checks and bundle never touch Expo or React Native.
- **Terminals start in a webview.** The first terminal is xterm.js in a webview with a key
  row for Esc, Ctrl, Tab and arrows. A native renderer on the core's own terminal engine
  can replace it if typing feels slow.

## Consequences

- Building the phone app needs rustup's Rust with the iOS and Android targets, Xcode, the
  Android NDK and `cargo-ndk`. Homebrew's Rust has no phone targets.
- The generated bindings and native libraries are build output, made by
  `pnpm native:ios` and `pnpm native:android`, not committed.
- `sikemux-mobile` compiles all of `sikemux-core`, including the server it never runs. If
  that becomes a size or linking problem on a phone, the server moves behind a Cargo
  feature.
- The phone and the core share one protocol version, so a mismatched pair says so at the
  handshake instead of misreading each other.

## Alternatives considered

- **Bare React Native CLI.** Rejected: it gives nothing a development build does not, and
  loses EAS and Expo's modules.
- **A TypeScript client.** Rejected: iroh does not run in React Native's JavaScript, and a
  second implementation of pairing and the protocol would drift from the core.
- **A separate repository.** Rejected: the phone would pin an older `sikemux-core` and fall
  out of step with the Mac without anything noticing.
