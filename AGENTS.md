# Repository Instructions

You are the owner and long-term maintainer of this codebase, and its design is up to you.

## Git workflow

- Work directly on the `main` branch for day-to-day work and nightly releases.
- Release branches are the only exception. Cut `release/<major.minor>` from `main`
  to stabilise a stable release, and cut hotfix branches from `release/*`.
- Never merge a release branch into `main`. Cherry-pick the fix commit instead, so
  version-bump commits stay on the release line.
- Do not create Git worktrees by hand. A teammate started with `agent_spawn` and
  `checkout: "own"` gets one from the core: work there, and land your commits with
  `git_commit` and `land: true`.
- Commit day-to-day changes directly to `main`.
- Make small, atomic commits as work progresses.
- Keep a commit's subject under 72 characters. Put the detail in the body, after a blank line.
- Other agents are always working in the same tree at the same time. Stage only the
  files your own session touched, by path, and commit those. Never `git add -A`,
  `git add .`, or `git commit -a`, and never stash, revert, or amend anything you
  did not write. Unrelated dirty files belong to someone else — leave them alone
  and do not mention them as blockers.

- Do not proactively write comments in code. We prefer code to be self explanatory. When we write comments its because there is something locally unintuitive that a future reader should know. But as we write code our goal is to make all code locally intuitive, removing the need for comments. If we ever do need to write comments, we never introduce jargon. Comments should be understandable to someone who was just dropped into the codebase for the first time. Comments should attempt to be concise, on average 1-2 lines. If you are writing a longer comment its likely there is a lot of useless information, which is bad because the information may become stale as the code changes
- Do not leave random markdown files in the codebase that are meant to be some way to deliver information to me. If you want to write a markdown file write it in a temporary file, and give me the path and chat and I can read it
- Never write code that is explicitly backwards compatible. Systems should handle backwards compatibility (like migrations), not logic. If there is some logic that needs to be written otherwise it would appear it would break older users, you MUST make the assumption that no users have ran that code yet and its unreleased, so it would not make sense to consider the side effects that code would produce. This is a safe assumption because the maintainers of this codebase always ensure code that gets shipped is compatbile with the systems that allow for us to not have to explicitly hardcode backwards compatibility

## Tests

- Do not write unit or integration tests on your own initiative. A test an agent writes
  is only its own reading of the task, the same reading the code came from, so it
  catches nothing the code would get wrong. Write one only when asked, or to reproduce
  a bug that actually happened.
- Never write a test that restates the implementation: same branches, same constants,
  mocks of everything it calls.
- Keep the checks that hold things we decided: the stale-output checks in `pnpm check`
  (`grammars:check`, `file-icons:check`, `release:check`, the server protocol), contract tests such as `mobile/app/test/updatePaths.test.ts`,
  and the end-to-end runs. Do not weaken or delete them to get a change through.
- When a change breaks an existing test, decide whether the behaviour it pins was meant
  to change. Update the test only if so; otherwise fix the code.
- Run every Rust build and check with the checkout's `scripts/bin` first on `PATH`
  (`export PATH="$PWD/scripts/bin:$PATH"` from its root; the Makefile does it). Its
  `cargo` runs rustup's Rust, the version in `rust-toolchain.toml` (Homebrew's older
  Rust ignores that file, so clippy there can pass code CI rejects), and shares compiled
  crates across checkouts through kache, or sccache, when one is installed. It runs only
  as many builds at once as free memory takes, starting them 20 s apart and none while
  macOS reports memory pressure: a run over that says it is queued and starts by
  itself, so never wait for other agents' cargo with `sleep` or `pgrep`. `SIKEMUX_CARGO_RUNS` sets the limit. An agent's builds run at
  utility priority, so the app stays responsive. Waiting runs go in order of arrival,
  except that the final checks before a commit go ahead of any other run that has waited
  under 15 minutes. `clippy` and `test` count as final checks; set
  `SIKEMUX_CARGO_PRIORITY=final` on another run you make just before committing, or
  `normal` on a `test` you run while still exploring.

## Speed and size

- What a person feels must never get visibly worse: startup, typing, scrolling,
  opening a pane, a terminal under heavy output. Making any of it faster is
  always welcome.
- It must feel fast under stress, not only when idle: many agents streaming at
  once, terminals flooding, a large repository, a long chat, a busy machine.
  Time changes in those conditions, since that is where slowness shows first.
- Size and speed trade against each other; make that trade only where nobody can
  feel it. Build cold code for size and keep hot paths (terminal output,
  serialization, search, git) at full speed. Measure both sides before choosing.
- A change that adds weight (a dependency, a font, an image, a crate in the
  core) says what it costs the DMG.

## Before and after pictures

- A picture never needs the person: no account, no paired device, no signed-in simulator. Never ask
  them to sign one in for a shot.
- Take pictures only when the look changes and the person will judge it. Shoot the phones only
  when a phone screen changed.
- `node scripts/shots.mjs mac <commit> <scene>…` draws the Mac window at the commit, from the
  `snap` steps of `film/scenes/<scene>.film`, on a throwaway core and the stand-in Claude. `phone`
  does the same for the Android app's showcase scenes (`mobile/showcase/capture.mjs`). `iphone`
  builds the native app in its showcase mode (`-SikemuxShowcase`, Debug builds only, from
  4819b46d8 on) on the same data, and prints the `sim` calls that take each picture; `--compare`
  then writes the page.
- Nothing is built at the parent. The "before" of each picture is the last one taken under the
  same scene name, so keep a scene's name when the change is to how it looks.
- Keep the data what a real core folded (the showcase's `recorded` chats, a scene's replies played
  through the core), never rows drawn by hand.
- It builds in one checkout kept for pictures, `~/Library/Caches/com.nodelike.sikemux/shots`, one
  run at a time, so after the first run builds are incremental. Take shots there after the land,
  not with a build of your own checkout. `compare.html` beside the pictures stacks each pair for
  `page_show`.

## Where the rest lives

Each of these folders has its own `AGENTS.md`; read it before working there.

- `mobile/`: the iPhone and Android apps.
- `server/`: the accounts backend, the web app and citadel.
- `helpers/notch/`: the island over the MacBook notch.
- `brand/`: every logo and icon. Take them from there; never draw or render the logo again.
- `.github/`: releases, nightlies and the download the site reads. Releases publish only from
  the Release workflow; never run `scripts/release.sh --publish`, and never approve a stable
  release for the owner.

Two places live outside this repo. sikemux.com is `nodelike/sikemux-front`, checked out at
`~/projects/personal/sikemux-front`; pushing its `main` deploys it. `marketing/` is a separate
local Git repo, ignored by this one: file references and contacts there and commit inside it.

## Protocol and clients

- The phone and the core share `sikemux-wire`'s protocol. A protocol change must keep
  `sikemux-mobile` building, and bumps `PROTOCOL_VERSION` once anything already released
  speaks the old shape. A type the phone's Rust does not act on goes in `sikemux-view`, which
  it does not link.
- The phone links only `sikemux-wire` and `sikemux-client`, so their code, not the
  core's, decides when an Android phone needs a store build rather than an over-the-air
  update; the iPhone app has no over-the-air updates.
  Keep them to data and transport: never make either depend on `sikemux-core`,
  `sikemux-pty` or `workspace-hack`. A new field rides inside JSON the bridge already
  passes through rather than on a typed UniFFI record. A request the phone needs is a
  method of the core's `Call` (`sikemux-core/src/server/calls.rs`), never a new wire
  request or bridge method, and a released method is never removed or renamed.
- The core owns what the data means; clients own only how it's shown. The core folds a
  chat into rows and sends their changes, and works out agent status, questions and the
  catalog. The Mac, the phone and every other client keep one store of what the core
  sends and draw it with their own words, icons, formatting and markdown drawing; none
  reads an agent's messages or repeats the core's logic. ADR 0010 records why.

## Docs

- The user docs for docs.sikemux.com live in `docs-site/`, a Fumadocs site on Astro with
  its own package, dark only in sikemux.com's palette. `pnpm dev` there serves them.
- A change that adds, removes or renames something users see carries one `Docs:` line in its
  commit body saying what changed for them. Leave the pages alone: a daily docs pass, a board
  card at 18:00, writes them from those lines, so the docs trail main by up to a day.
- The pages under `reference/` are generated: run `pnpm docs:reference` after changing
  a default shortcut, the CLI's help or `tools/tools.json`. CI fails while they are
  out of date.

## UI rules

- sikemux.com's copy must stay true of the shipped app and its look follows `DESIGN.md`. When a
  change adds, removes or renames something the site describes, say so, so the site
  can be updated.
- Follow `DESIGN.md`. It is short on purpose: keep it to principles and tokens,
  and change it in the same commit when the look moves on.
- A Mac UI change that the phones also draw lands on the Mac alone. A daily phone port, a board
  card at 18:00, brings the day's Mac changes to both phone apps, so the phones trail the Mac by
  up to a day.
- The notch island's agent rows mirror the tab rail's rows (`app/src/agents/mod.rs`): when
  the rail's row changes, change the island's to match.
- A page or mockup of the app takes its Figtree from `helpers/notch/Fonts` by local
  path; `page_show` inlines it.
