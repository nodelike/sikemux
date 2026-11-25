# ADR 0027: A preview opens by the releases a host follows

- Status: Accepted
- Decision date: 2026-11-25

## Context

Team opens to people on nightly first. A stable release is a nightly build promoted unchanged, so
nothing compiled into the app can keep Team shut on stable.

## Decision

The accounts server decides, at runtime. The app, the web app and the phones show what it says.

- A host says which releases it follows in a request signed with its key, each time it connects
  to the account and each time the person changes the setting. What it says holds for 30 days. It
  is the setting that counts, not the build that happens to run.
- `team_audience` holds who Team is open to: staff, people on nightly, or everyone. It ships at
  staff, is at nightly since 25 November 2026, and setting it back to staff shuts Team again.
  Whether orgs' vaults are open to people outside staff is a setting of its own,
  open since 25 November 2026.
- Who may use Team and whether payments are real are separate. Turning payments on opens Team to
  nobody.
- An org made in the preview has 20 free seats and is an ordinary org. It carries over when
  payments go live.

Who sees what:

- **Both channels on one account.** One host on nightly opens Team to the account. A host that
  follows stable is told its orgs are hidden: it shows nothing of them and uses none of their
  keys, and keeps what it holds.
- **Someone who moves to stable.** The app says first what they would lose. Their account keeps
  reading its orgs, leaving them, removing members and handing them over, on the web. Nothing is
  deleted, and following nightly again brings it all back.
- **Someone on stable who is invited.** They are told to choose nightly on one of their hosts.
- **The web app and the phones.** They have no channel, so they go by the account.
- **A host too old to say.** It is told only of orgs whose vault key it holds, hidden.
- **Staff.** Always in, on any channel.

## Consequences

- A changed build can claim to follow nightly. This decides who sees a preview and protects
  nothing: membership, roles and the vault's keys are checked as they are for everyone.
- Before payments go live, owners of preview orgs get at least 30 days to pick a plan, the free
  seats get an end date, and an owner who pays while on stable must read as paid.
