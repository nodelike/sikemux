# ADR 0025: Agents have an identity of their own, and the core holds its key

- Status: Accepted
- Decision date: 2026-11-15
- Amended: 2026-11-15, to cover repositories where no App can be used, more than one App, and
  keys that stay on one computer
- Builds on: [ADR 0010](./0010-phone-links-only-the-wire-and-client.md), [ADR 0016](./0016-the-account-vault.md)

## Context

Agents ran as the person on GitHub. Every push used the person's SSH key, and every agent
started with the person's token in its environment, with `repo` and `workflow` on it. A second
token in `gh`'s keyring added `delete_repo`.

- A rule that named the owner as its only bypass actor stopped nobody, because every agent was
  the owner. That covered the rules on `main`, on release tags and on release branches, and the
  reviewer of the `approval` environment.
- A commit's author, committer and pusher were all the person. Only the signed `Agent:` footer
  said which agent wrote it.
- An agent could change a signing job or the script it runs, and the next nightly would run it
  with the signing keys in reach.
- GitHub puts releases under the same permission as pushing code, and no rule or setting
  splits them. A token that can push `main` can also mark a nightly as the latest release,
  which the site's Download button and the stable update feed read.

Agents run as the person's Unix user, so a file the person can read, an agent can read. Three
places hold a secret against that: the core's keychain store, a service that issues short
tokens from a key kept there, and something the person physically does.

The first decision tied the core's push to an App. A repository with no App fell back to agents
holding the person's token. Two kinds of repository were left there. One belongs to an
organization, which needs an App of its own. The other can have no App at all, because the
organization refuses Apps or the host is not GitHub. A key for work also must not ride the
person's own account vault to the computers they keep at home.

## Decision

- **A GitHub App is the agents' identity.** Each person creates their own from Sikemux through
  GitHub's manifest flow and installs it on the repositories agents work in. No key is shared
  between Sikemux users and the accounts server sees none. An App was chosen over a second
  account because its tokens last an hour and narrow to one repository, it has no
  administration rights to give away, and it cannot review an environment.
- **The core holds the key and mints.** The key is a sign-in in the GitHub plugin's keychain
  store, carried to linked hosts by the account vault. Each host mints for itself. A token
  never travels through a plugin call, which mods, clients and agent tools can reach by name.
  The plugin contract has four methods only the core calls. They mint a credential, revoke
  one, merge a pull request as the App, and say who the core pushes as on a remote.
- **No token an agent can reach writes a repository's contents.** The stand-ins for `git` and
  `gh` ask the core, and the core answers only a caller whose process tree leads to an agent
  it started. That caller gets a one-hour token for the one repository that reads contents
  and writes pull requests, issues and Actions. With an App on a project's remote, its agents
  no longer get the person's GitHub token.
- **The core makes every push.** An agent's `git push` runs unchanged against a bare
  repository the core makes for that push, so git itself works out the refs, checks the
  leases and runs the pre-push hook. The core then sends exactly the refs that moved to
  GitHub, as a `git` of its own that reads no config and runs no hook from the agent's
  checkout. Only that `git` gets a token that writes contents, minted for the one push and
  revoked when it ends. The core never forces a ref the agent's own push did not force.
  `git_commit` and the merge queue push the same way. `gh pr merge` is handed to the core,
  which merges inside the plugin as the App.
- **One App for each owner.** A private App installs only on the account that owns it. So a
  person who also works in an organization has their own App and one the organization owns,
  made by one of its owners through the same flow. The core keeps every App it was given and
  uses the one installed on the repository.
- **Without an App the core still pushes, with the person's sign-in.** When the person has
  signed in to the host's connector and that sign-in can push, an agent's `git push` takes the
  same path into the core, and the core's own `git` sends it with that sign-in. Only that push
  and the fetch that fills the core's repository for it get the sign-in. Nothing else the core
  runs does. On Bitbucket that is an OAuth token that lasts two hours, or a repository access
  token when the person signed in with one. Bitbucket shows such a token as a bot of the
  repository's own.
- **There, agents hold no token at all.** Bitbucket's scope for writing pull requests includes
  pushing, so no token can be handed to an agent's commands. Pull requests go through the
  connector's tools, which the core runs. Fetching stays on the person's own git setup.
- **A key can stay on one computer.** A sign-in marked as kept on this computer never enters
  the account vault, and one already in it leaves. A computer can make that the rule for every
  new sign-in. An App an organization owns starts out kept. The mark is the person's to set,
  since Sikemux cannot tell which sign-ins are for work.
- **The person stays the author.** The App's bot is the committer and the pusher. Rules look
  at the pusher, so they apply to agents.
- **The core alone pushes workflow changes, and asks only about ones that can reach
  secrets.** GitHub's `workflows` permission covers the whole folder, so no token an agent
  can reach carries it and the split lives in the core. A commit goes out without asking when
  every workflow file it touches is plain before and after it. A plain file is a regular file
  of printable ASCII that sets `permissions` at the top level, names no environment, no
  `id-token`, no secret but the built-in token and no other repository's workflow, and gives
  its own token no write to contents. Any other workflow commit waits for the person. One ask
  covers everything an agent has waiting, and a waiting commit holds up nobody else's pushes.
- **Scripts that run beside a key are pinned.** `pinned-files.yml` lists their hashes, and
  every job that names `release` or `production` needs it. Changing such a script means
  changing that workflow, so it reaches a release through the same pass.
- **Four gates stay, each enforced by a system.** A stable release and a phone promote, on the
  `approval` environment, with the release page out of reach of any token an agent holds. A
  change to a workflow that can reach secrets, or to a pinned script, on the core's pass.
  Deleting or moving a release tag, deleting the repository and changing its rules, secrets or
  environments, which no agent-reachable token can do. Speaking as the person on GitHub, where
  agents show as the bot.

## Consequences

- Landing and pushing `main`, nightlies, deploys and pull request triage stay the agents' own.
  A repository the core holds no credential for behaves as before.
- `gh release` and every other write to contents by an agent fails at GitHub. Releases come
  only from the Release run. A workflow's own token is covered by the plain rule, since a file
  that lets it write contents waits for the person.
- A push makes two hops, one into the core's repository and one to GitHub.
- The core reads a workflow's text and does not parse it, so nothing rests on reading YAML the
  way GitHub does. A file that only looks as if it mentions secrets waits too.
- The plain rule rests on the repository's default for workflow tokens being read-only,
  because a file could fake its `permissions` line inside a quoted string. It is read-only on
  nodelike/sikemux.
- The split keeps the person out of CI changes. On the 14 days before this decision 70 commits
  changed a workflow. 40 of them would have waited, in about 23 asks.
- The first half stops slips and overreach. An agent that goes looking still finds the
  person's own keys on the Mac, and the person's `gh` sign-in and signed-in browser still
  reach the release page. That lasts until those keys move behind Touch ID or off the disk,
  which is the person's to do once the App path has run for some days.
- Moving those keys does not stop an agent set on getting past the core. Agents run as the
  person, so one can change the files the core's own `git` runs, or read the name of a push
  in flight from the process table, and get the token meant for the core. Running agents
  contained closes that. It is separate work.
- The pinned scripts are not on `main` yet. They wait for the person's yes, and until then an
  agent can change a script a signing job runs.
- Where the core pushes with the person's sign-in, commits and pushes still show the person,
  so a rule that names the owner does not tell agents apart there. A repository access token
  gives Bitbucket a pusher of the agents' own, which its branch restrictions can name.
- On such a repository an agent still fetches with the person's own key, and that key can
  push. Sending pushes through the core keeps the core's credential from agents. It does not
  take the person's key from them. To move that key, fetching has to go through the core too,
  or use a token that only reads. Neither is built.
- A pushed change to `bitbucket-pipelines.yml` runs with the repository's secured variables.
  The core's pass over workflow files reads GitHub's only.
- A GitHub organization that refuses Apps is not covered. Agents keep the person's token
  there, because `gh` needs one and GitHub cannot narrow it.
- An organization whose owner does not use Sikemux cannot hand over an App made by hand yet.
- Sign-ins that went into the vault before they could be marked stay there until the person
  marks them.
- AWS, Google Cloud and citadel can take the same shape, a stand-in that gets a short
  credential from the core. They are not built.
