# ADR 0007: Paired devices reach the core over iroh

- Status: Accepted
- Decision date: 2026-10-02

## Context

A phone app should show the terminals and agents running on a person's Mac, answer
agents' permission requests and start new work, from anywhere, with the Mac's window
closed. Later the same client should reach a person's own Linux machines and machines we
host. The core already owns every session and outlives the window ([ADR 0006](./0006-background-core.md)),
but it only listened on a Unix socket that anything running as the person could use.

A phone is usually behind NAT on another network, so reaching the Mac needs either a
relay or hole punching, and the connection must be encrypted end to end so a relay never
sees terminal bytes or prompts.

## Decision

- **Transport: iroh.** The core runs an iroh endpoint (QUIC) keyed by its own ed25519
  key. Devices dial that key; iroh finds the core through n0's discovery, punches through
  NAT when it can and falls back to a relay when it cannot. Both ends are authenticated
  by their keys during the handshake. The phone app will use the same Rust client through
  native bindings, so there is one client implementation.
- **Off by default.** Settings, Devices turns remote access on. While it is on the core
  does not exit when idle.
- **Trust is a list of device keys.** `<socket>.remote.json` (mode 0600) holds the core's
  key, the switch and the paired devices. A device is added only by pairing, and revoking
  it ends its open connections at once.
- **Pairing: a six-digit code and the person's approval.** The device types the code
  shown on the Mac. Both sides run SPAKE2 on it, bound to both keys, so the code never
  crosses the wire, each guess costs a live attempt (five per code), and a device that
  dialled an impostor finds out before it sends anything. The person then allows or
  declines the device and picks its access.
- **Access.** Every request names what it needs. A device that watches may read sessions
  and answer permission requests; a device with full control may drive sessions and start
  chats; only the app on the Mac reaches the core itself (updates, stop everything,
  configuration, remote access and pairing). Access is checked on every request, so a
  change applies to open connections.
- **Starting work without the window.** The app publishes its open projects and, for each
  agent it can already start, the program and environment it would run. The core keeps
  these in memory only, because the environment can hold API keys; devices see only their
  names. Sessions and chats record which device started them, and the app's launch sweep
  leaves those alone.
- **Attention.** When an agent waits on a permission answer, every client hears it,
  whether or not it shows that agent. Push notifications will forward the same event.

## Consequences

- The sidecar grows from about 5.3 MB to 9.8 MB, or about 2.4 MB compressed, taking the
  DMG from about 10 MB to about 12.4 MB.
- While remote access is on, the core publishes its address to n0's discovery service and
  uses n0's public relays. Both can be replaced with our own servers without changing
  devices' trust.
- Remote connections drop during an in-place update; devices reconnect and take their
  sessions back with a fresh snapshot, as the app does.
- While remote access is on, a LaunchAgent starts the core at login
  (`src-tauri/src/login_item.rs`), so the Mac is reachable after a restart without
  Sikemux being opened. It restarts the core only after a crash; turning remote access
  off removes it.
- While remote access is on, the core also advertises itself over Bonjour, and a
  pairing code comes with a QR code holding the core's key, so a phone can find the Mac
  before it has paired.

## Alternatives considered

- **Noise over TCP with our own relay.** Much smaller, but no hole punching, so every
  keystroke would go through a relay, and we would build discovery and relaying ourselves.
- **Accounts as the root of trust.** Rejected: a stolen account would be a remote shell on
  the person's Mac. Accounts may later help devices find machines; trust stays with device
  keys approved on a machine the person already controls.
