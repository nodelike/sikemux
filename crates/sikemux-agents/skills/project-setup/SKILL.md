---
name: project-setup
description: "Set a project up so every checkout of its own runs apart from the others: write sikemux.json's steps, ports, env and heavy commands, then prove them on fresh checkouts with setup_check. Load when told a project's setup is not proved, or asked to set a project up."
---

# project-setup: make every checkout run on its own

Agents in checkouts of their own share the project's ports, database and build machine until
its `sikemux.json` says how a checkout gets its own. You write that, prove it, and commit it.
The person decides nothing here; they are asked once to trust the new steps.

## 0. Ask for the check

Call `guide` with topic `setup`. That lists `setup_check` among your tools, which only an
agent setting a project up is given.

## 1. Read the project

Read what already says how it runs, before writing anything:
- the README and any contributing guide;
- the manifests: `package.json` and its lockfile, `Cargo.toml`, `pyproject.toml`, `Gemfile`,
  `go.mod`, `pom.xml` or `build.gradle`;
- a `Procfile`, `docker-compose.yml` or `compose.yaml`, `.env.example`;
- the CI workflow, for the commands that prove the project and what they need first;
- any `sikemux.json` already there, and `workspace_inspect` for the processes Sikemux found.

Note each server and the port it listens on, each service it needs (a database, a cache, a
queue), what a fresh clone must run before it works, and which commands are slow or use much
memory.

## 2. Write sikemux.json

Keep what is there and add to it. The `config` topic of the guide has every field.
- `tasks`: each server, its command taking the port from `$PORT`, and `port` set to the port it
  uses in the project's own folder.
- `ports`: any other port a checkout needs its own of, such as a database's, by name.
- `env`: the values that must differ per checkout, with `{port.<name>}`, `{checkout}` and
  `{root}`, such as `"DATABASE_URL": "postgres://localhost:{port.db}/app_{checkout}"`.
- `worktree.onCreate`: what a fresh checkout runs, in order: installing packages, making its
  database, migrating, seeding. Each step must work in a checkout that has nothing yet.
- `worktree.onRemove`: what undoes them, such as dropping the database or stopping its
  containers, so nothing is left once the checkout goes.
- `heavy`: the slow commands CI runs, such as the tests and the build, `final` on the ones run
  before a commit. Never list a dev server or a watcher: a heavy command must end.
- `check`: the command that proves the project, when it has none.
- Something that cannot have a copy per checkout, such as a hard-coded port or a sign-in
  redirect address, stays on the project's own port; say so in your answer.

## 3. Prove it with setup_check

Call `setup_check`. Sikemux cuts two fresh checkouts at once, runs the steps in both, starts
every process with a port and waits for it to answer, runs the check, then runs the removal
steps, removes both and confirms nothing is left listening. It answers each step with passed
or failed, the last line of a failed one and its log.

## 4. Fix until it passes, then commit

Read the failed step's log, fix `sikemux.json` or what it runs, and call `setup_check` again
until every step passes. Two checkouts at once is the point: a step that only works once, such
as one that takes a fixed port or a fixed database name, fails here. Then commit `sikemux.json`
and answer in plain words what each checkout now gets and which commands take turns.
