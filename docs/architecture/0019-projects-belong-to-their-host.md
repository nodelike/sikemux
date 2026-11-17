# ADR 0019: Projects belong to their host

- Status: Superseded by [ADR 0023](./0023-projects-are-repositories-with-a-copy-per-host.md)
- Decision date: 2026-10-22
- Supersedes: [ADR 0017](./0017-projects-belong-to-no-host.md)

## Context

ADR 0017 showed each repository once, on every client, with a copy on each host that held one,
and listed chats and terminals from every host together under it. ADR 0018 then put the person on
one host at a time, chosen in the rail beside the project, with the tab rail listing what runs on
that host. The two pull apart: with a server chosen, the project list still showed the laptop's
projects, most of which have no copy there, and a project only the server holds sat in the same
list with nothing to say so. Showing a repository once also needed a layer of its own: knowing two
checkouts are one repository, cloning on first use, and a copy chip in the Files page.

A server reached over SSH that does not run Sikemux has no projects, chats or agents to show at
all, only a shell.

## Decision

- **A project is a folder on one host.** The Projects group lists the chosen host's projects.
  Choosing another host shows its projects, and each host keeps the project the person last had
  open there.
- **Working on the same repository elsewhere is an action.** A project's menu offers **Open on
  <host>** for each other Sikemux host: the host clones it from its remote with its own sign-in, or
  takes it from this host as a git bundle when it cannot reach the remote, then the person is
  switched to that host and project. Workers on another host keep getting their code as ADR 0013
  describes.
- **Clients reach each host directly**, as ADR 0017 decided: the desktop app joins every host on
  the account as a device of its own over iroh, and files, search, language servers and previews
  run in the core of the host that holds the project.
- **A host without Sikemux is a shell.** Choosing an SSH host that does not run Sikemux shows a
  terminal on it, opening one when none is open. The top bar, where the project picker sits,
  offers **Install Sikemux on <host>** instead. Chats, agents and every other pane that needs a
  core are not offered there: the new-pane menu holds an SSH terminal and an **Install Sikemux**
  pane, which explains what installing adds and runs the installer over that SSH connection, as
  the host's menu does.

## Consequences

- A person with one host sees no change.
- Two checkouts of one repository on two hosts are two projects, one under each host.
- The project identity, first-use cloning and copy chip of ADR 0017 are not built.
- What the person sees on a server is its own: its projects, its chats and terminals, its files.

## Alternatives considered

- **Projects shown once with a copy per host (ADR 0017).** Rejected: it hides where the code is
  while the person has chosen a host, and needs matching and cloning machinery to do it.
- **Every host's projects in one list, each marked with its host.** Rejected: with a host chosen,
  the other hosts' projects are rows that open onto nothing.
