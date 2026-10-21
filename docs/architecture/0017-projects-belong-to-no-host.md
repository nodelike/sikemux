# ADR 0017: Projects belong to no host, and each chat runs where the person picks

- Status: Accepted
- Decision date: 2026-10-20

## Context

Every client is a way in to any host on the account
([ADR 0014](./0014-agents-run-on-hosts-and-every-client-is-a-way-in.md)), but a project is still
a folder on one host. A person with a laptop and a server would see the same repository twice,
once per host, and have to remember which copy holds what. Workers on a linked host
([ADR 0013](./0013-hosts-link-to-run-agents-for-each-other.md)) already get the code they need
without that: the host clones from the remote, takes commits it lacks as a git bundle, gets the
lead's uncommitted changes as a commit on top, and sends its own commits back on a branch.

Other tools ask the person to choose the machine first, a window per remote host or a host
picker over the project list, and so show each project once per machine.

## Decision

- **A project is its git repository, not a folder.** Its identity is its remote, normalised so
  that SSH and HTTPS addresses of one repository match (`github.com/nodelike/sikemux`). A
  repository with no remote is known by its first commit, which every copy shares, including
  copies sent as a bundle. A folder that is not a git repository stays a project of the one host
  that holds it.
- **Each host may hold a copy.** A copy is an ordinary checkout on that host. The project shows
  once in every client, wherever its copies are.
- **A chat or terminal runs where the person picks.** The new-chat strip, beside the project and
  the worktree switch, gets a host chip: `on This Mac ▾`. It shows only once the account has a
  second host, and defaults to the host the person last worked on the project on. A chat or
  terminal keeps its host for life, and says `on test-ec2` wherever it is listed when that is
  not this host.
- **A host without a copy gets one when it is first picked.** The chat says where the code comes
  from and starts once the copy is ready:
  - from the remote, cloned with the person's GitHub or Bitbucket sign-in for a private
    repository, added by the core from the vault
    ([ADR 0016](./0016-the-account-vault.md)) and never written into the copy;
  - from the host the person is on, as a git bundle, when there is no remote or the host cannot
    reach it;
  - with the person's uncommitted changes as a commit on top, unless they turn that off.

  A folder that is not a git repository cannot be picked on another host; the chip greys the
  others out and says why.

- **Work moves between copies as commits, never by syncing files.** A chat on another host
  brings its commits back on a branch named for the host and the chat, as workers do; pushing
  and pulling the remote works as it always has. Hovering a project shows each copy's branch,
  how far it is ahead or behind, and whether it has uncommitted changes.
- **The rail does not split by host.** Under each project, chats and terminals from every host
  are listed together. The Files page shows the copy where the focused chat or terminal runs,
  with a copy chip at its top only when the project has more than one.
- **A host's health shows on what it runs.** A chat, terminal or chip on a host that dropped
  says it is reconnecting; Settings, Devices keeps each host's load, memory and last seen, and
  adding a server.
- **Clients reach each host directly.** The desktop app connects to every host on the account
  over iroh as a device with full access, as the phone does, not through its own core. Files,
  search, language servers and previews run in the core of the host that holds the copy, as
  methods of the core's `Call`, so every client and the phone share them. The window, its
  browser tabs ([ADR 0015](./0015-one-browser-per-account-seen-from-every-device.md)) and
  rendering stay in the app.

## Consequences

- A person with one host sees nothing new: no host chip, no copy chip, no labels.
- The project list is the account's, so it lives in each core's workspace keyed by project
  identity rather than by path, and a host adds the path of its own copy.
- Clones of one repository on one host are one project; parallel copies are worktrees.
- A new host is useful at once for public repositories and local ones; private repositories on
  a host that has no sign-in of its own need the vault, or the bundle from the host the person is
  on until the vault exists.
- Each core serves file, search and language-server calls for its copies, so a remote project
  costs the Mac only the window, and typing in a local terminal never waits on another host.

## Alternatives considered

- **A window per host**, as remote editors do. Rejected: the inbox, the board and teams that
  span hosts would split across windows.
- **The project list grouped by host, or one page of the rail per host chosen from a footer.**
  Rejected: the same project shows once per host, and the person has to know which copy holds
  their work before they can start.
- **Projects as folders synced between hosts.** Rejected: syncing live files both ways loses work
  when two copies change, and moves build output and secrets that should stay put.
