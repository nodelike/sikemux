# ADR 0023: Projects are repositories, with a copy on each host that holds one

- Status: Accepted
- Decision date: 2026-11-10
- Supersedes: [ADR 0019](./0019-projects-belong-to-their-host.md)

## Context

ADR 0019 made a project a folder on one host and listed the chosen host's projects. It
rejected ADR 0017's single list for two reasons: with a host chosen, the list still showed
projects that had no copy there, and showing a repository once needed matching and cloning
machinery.

People now keep one repository open on more than one host at once: the laptop and a server
such as citadel, with agents on both. Under 0019 that is two projects the person flips
between by switching host, and a lead on one host and its workers on another sit in two
lists. The matching 0017 needed is already built: the core keeps `ProjectInfo.identity`
(`crates/sikemux-core/src/git/remote.rs::project_identity`), and ADR 0013 already moves code
between hosts for workers.

## Decision

- **A project is a repository.** Its identity is the one the core already keeps: the remote,
  normalised, else the first commit. A folder opened inside a repository, such as one package
  of a monorepo, adds its path within the repository, so it stays a project of its own.
- **Each host may hold copies.** A copy is an ordinary checkout on one host. Two clones on one
  host are two copies of one project, told apart by folder; worktrees stay worktrees of their
  copy.
- **The core folds the list.** Each core lists its own copies and, over the host link
  (ADR 0013), its linked hosts' copies of the same repositories, and sends clients one project
  per identity with its copies: host, folder, branch, ahead and behind, uncommitted changes.
  Order, pins and spaces are kept by identity, so they follow the repository. Clients only
  draw it.
- **The project list does not change with the host.** The Projects group lists every
  project on the account once. A project held only elsewhere shows that host's name on its
  row; a project held on several hosts shows how many. Choosing a host in the Hosts group no
  longer swaps the list: it goes to the copy in front there, and an SSH host without Sikemux
  still opens a shell on it as 0019 decided.
- **Each project has a copy in front.** It defaults to the copy on the host the person last
  worked on it on, else this host. It is chosen in the project's tree, at the top, only when
  the project has more than one copy. Files, Git, Search and a new terminal or chat use the
  copy in front.
- **A chat or terminal keeps its host for life and says so.** The agents rail lists the
  project's chats from every host in one ranked list, leads with their workers wherever each
  runs, and names the host on any row not on this host. The New chat and New terminal rows
  name the copy in front's host when it is not this host.
- **Files shows one copy.** Its header carries a copy chip, `on citadel ▾`, only when the
  project has more than one copy; the chip switches the copy in front.
- **A host with no copy gets one on request.** The copy switch and the project menu offer
  **Copy to <host>** for each linked host without one: cloned from the remote with that
  host's own sign-in, else taken from the copy in front as a git bundle, always into a new
  checkout and never one the host already has. Its uncommitted changes go too, as a commit on
  top the way workers get their lead's (ADR 0013), unless the person turns that off in the
  copy switch. The new copy joins the project and becomes the copy in front. Work moves between copies as commits, as workers'
  does, never by syncing files.
- **A folder outside git is a project of one host.** It has no identity, shows with its
  host's name when that is not this host, and offers no **Copy to**.

## Consequences

- A person with one host sees no change: no copy switch, no host names, no chip.
- 0019's objection does not arise, because no host choice filters the list; where the code is
  shows on the row, in the tree and on every chat that runs elsewhere.
- `ProjectInfo` gains its copies, and the device view carries them, so the phone shows the
  same list. Projects of a linked host that is offline keep their last known copies, marked
  unreachable.
- The glossary's "project" becomes "a repository, with a copy on each host that holds one".

## Alternatives considered

- **Projects listed per host (ADR 0019).** Rejected: a repository open on two hosts is two
  projects, and a team spanning hosts splits across them.
- **A host switch filtering one list.** Rejected: the list depends on a choice made elsewhere,
  and a lead on one host and its worker on another split across views.
- **Copies as rows of the project's tree, and the agents rail grouped by host.** Rejected: the
  tree grows a level, and the groups split a lead from its workers.
- **Grouping in each client.** Rejected: the core owns what data means (ADR 0010), and the
  phone would have to repeat it.
