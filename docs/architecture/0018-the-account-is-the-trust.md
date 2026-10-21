# ADR 0018: The account is the trust

- Status: Accepted
- Decision date: 2026-10-21

## Context

A host lets a device in only when its owner allows it
([ADR 0007](./0007-remote-access-over-iroh.md), [ADR 0009](./0009-accounts-and-backend.md)), and
two hosts work for each other only once the owner allows the link
([ADR 0013](./0013-hosts-link-to-run-agents-for-each-other.md)). The account only suggests; the
host decides. That way a stolen account or a broken-into accounts server can ask to be let in but
cannot get in.

In use, that rule costs every person, every time, on every host. Adding a Linux server and opening
it from the desktop app took four steps: sign the server in, link the laptop to it from the
server's terminal, turn on a switch to show it in the app, then allow the same laptop a second
time from a phone, three taps deep, before the request expired. The person had already proved who
they were by signing both in to the same account.

Products people trust with remote shells mostly make the other choice. Tailscale puts a machine
on the network once it signs in to the account, and offers Tailnet Lock, where trusted devices
must sign each new one, to those who want it. VS Code's remote tunnels and GitHub Codespaces open
for whoever is signed in to the GitHub account.

## Decision

- **Signing in is enough.** A device or host signed in to the account is trusted by every host
  on that account, with full access, as the person. A host shows up in every app on the account
  as soon as it signs in, and every host on the account is linked to every other. There is no
  pairing, no link to allow, and no switch to show a host in the app.
- **The join ticket is the proof.** Devices and hosts still join with the account's signed join
  ticket of ADR 0009, and the host still checks it itself: the signature, the expiry, the key it
  names and that the account is its owner's. A valid ticket now lets the device straight in
  instead of asking.
- **The account guards the door.** Signing a new device or host in to the account takes a code
  sent to the account's email, a provider's sign-in, or approval at app.sikemux.com. Every device
  the person already has hears "New device on your account: nodelike's MacBook Pro", with a
  **Remove** that revokes it on every host at once, as removing a device already does.
- **A lock for those who want the old rule.** Each host keeps a **Require approval for new
  devices** setting, off by default. While it is on, the host asks before letting a device in as
  ADR 0009 described, and the request reaches the person as a notification and a card on every
  device acting as them, and waits ten minutes for an answer. The setting lives on the host, not
  the account, and only the person changes it, so a broken-into server cannot turn it off.
- **Hosts are one group in the rail.** The rail's SSH group becomes Hosts: this host, every host
  on the account, and each SSH host while the person has it open, one row each, with its state,
  and the account's host matched to its SSH entry when one installed the other. Any host in
  `~/.ssh/config` is opened from the group's picker, as before.
- **A host is chosen as a project is.** Choosing a host's row shows, in the tab rail, every chat
  and terminal running on it, from every project; choosing a project shows the project's from
  every host. One row in the side rail is chosen at a time. Work starts on a host the person can
  see: the chosen host, else the host of the focused chat or terminal, else this host. That host's
  row says "here", so with a project chosen the person still sees where ⌘N and ⌘T will start, and
  the new-item menu, which keeps three rows, names it when it is not this host. A row's menu
  starts a chat or terminal there, through the core on a Sikemux host and over SSH on any other,
  opens its files, renames or removes it. An SSH host's menu offers **Install Sikemux here**,
  which runs the installer over that SSH connection and signs the host in to the account by
  itself.
- **An agent uses the sign-ins of the host it runs on, whoever started or messaged it.** ADR 0013
  refused plugin calls and the browser to any agent that an agent on another host had started or
  messaged, and to every lead above it, until the person allowed that host. That rule goes: the
  person's hosts are one place to them, and a lead that hears from its worker on a server keeps
  its tools.
- **What stays separate.** A device signed in to another account still asks the owner, and can be
  limited to watching. The vault stays sealed from the server
  ([ADR 0016](./0016-the-account-vault.md)), and agent CLIs still sign in on each host.

## Consequences

- Anyone who gets into the person's Sikemux account, or into the accounts server, can run
  commands on every host on that account. That is the trade this decision makes, the same one
  Tailscale and VS Code tunnels make. The lock is the answer for a host that cannot accept it.
- The accounts server's ticket signing key is now as sensitive as a key to every host. It must be
  kept apart from the API's other secrets, rotated on any suspicion, and hosts must accept a new
  one without being signed in again.
- `sikemux pair`, the link prompt, the app's link step, "Show … in this app" and the new-item
  menu's rows for each host go away. A headless host is set up by `sikemux login` alone.
- Hosts link to every other host on the account automatically, so an agent on any host can start
  workers on all of them, and a worker on a host uses that host's sign-ins.
- An agent steered by what it read, a poisoned issue or web page, can act as the person with the
  sign-ins of the host it runs on, wherever the text came from. ADR 0013's rule never caught that:
  an agent on this host reading the same page kept every sign-in. Guarding against it is the
  sandboxes' job ([ADR 0014](./0014-agents-run-on-hosts-and-every-client-is-a-way-in.md)), not the
  host an agent's message came from.
- Settings, Devices lists the account's hosts and devices with what they are doing, and removing
  one from there revokes it everywhere.

## Alternatives considered

- **Keep the host's approval, but deliver it better.** Requests as notifications that wait, and
  one approval covering both a Mac's link and its app. Rejected as the default: it still asks the
  person to prove twice what signing in already proved. It remains as the lock.
- **Trust the device that entered the host's sign-in code, and ask for every other.** Rejected:
  the person's next device and every new host still meet the old prompts.
- **Keep refusing sign-ins to agents reached from another host.** Rejected: it stripped a lead of
  its browser and plugins the first time its own worker on a server reported back, and guarded
  only against text that crossed a host, not the same text read here.
- **The lock as an account setting.** Rejected: the server that the lock guards against could
  turn it off.
