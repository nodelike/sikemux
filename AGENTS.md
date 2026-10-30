# Repository Instructions

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
  (`grammars:check`, `file-icons:check`, `release:check`, the notch icons,
  the server protocol), contract tests such as `mobile/app/test/updatePaths.test.ts`,
  and the end-to-end runs. Do not weaken or delete them to get a change through.
- When a change breaks an existing test, decide whether the behaviour it pins was meant
  to change. Update the test only if so; otherwise fix the code.
- Run every Rust build and check with rustup's Rust first on `PATH` (`cargo --version`
  shows the version in `rust-toolchain.toml`). Homebrew's older Rust ignores that file,
  so clippy there can pass code CI rejects.

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

## Releasing

- Follow `docs/releasing.md`. Releases publish only from the Release workflow, started by
  pushing a `v*` tag; never run `scripts/release.sh --publish` yourself, and never approve
  a stable release on the `approval` environment for the owner.
- Nightlies are tagged on `main`. Stable releases and hotfixes are tagged on
  `release/<major.minor>`, and their version bump and notes go only there.
- Never check out `release/*` in this shared checkout. Commit to it from objects with a
  temporary index, as `docs/releasing.md` shows.
- After a stable `0.x.y`, `main`'s nightlies build toward the next release:
  `0.x.(y+1)-nightly.N`, or `0.(x+1).0-nightly.N` for a bigger one. A hotfix is that next
  release shipped early: fix on `main`, ship a nightly, promote it. Cherry-pick onto
  `release/*` only in an emergency, as `docs/releasing.md` describes.

## Mobile app

- The phone app is in `mobile/` (Expo, `mobile/app`) with the core's Rust client bridged
  in `mobile/native` from `crates/sikemux-mobile`. `mobile/` is its own pnpm
  workspace: never add it to the root install, scripts or checks.
- The phone and the core share `sikemux-wire`'s protocol. A protocol change must keep
  `sikemux-mobile` building, and bumps `PROTOCOL_VERSION` once anything already released
  speaks the old shape.
- The phone links only `sikemux-wire` and `sikemux-client`, so their code, not the
  core's, decides when a phone needs a store build rather than an over-the-air update.
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
- TypeScript the phone and the share page both use lives in `shared/`, imported as
  `@shared/*`. It is plain source, not a package: each workspace that imports it resolves
  its packages from its own install, and its tests run under the phone's vitest.
- Phone builds need rustup's Rust first on `PATH` as well; Homebrew's has no phone
  targets.
- The bindings `uniffi-bindgen-react-native` generates are build output; do not commit or
  hand-edit them.
- Phone screens are designed in `mobile/design/screens.src.html` before they are built, and
  it must keep matching the app. Change it in the same commit as the screen it draws; run
  `pnpm design` in `mobile/` to view it.

## Server

- The accounts backend is in `server/`: the API (`server/api`), the web app at
  app.sikemux.com (`server/app`), the shared protocol (`server/protocol`) and what runs
  them on citadel (`server/deploy`). It is its own pnpm workspace: never add it to the
  root install, scripts or checks. Run `pnpm check` in `server/`.
- The protocol is generated from `server/protocol/schema`. Change the schema and run
  `pnpm protocol:generate`; the TypeScript, the OpenAPI document and the core's
  `accounts/protocol.rs` are build output that CI compares against it. Within `/v1` the
  schema only grows: never remove or rename a field, route or enum value a released app
  reads.
- Migrations in `server/api/migrations` are never edited once merged. A change that
  removes something comes in two steps: stop using it in one release, drop it in a later
  one, so a rollback always finds the schema it expects.
- `server/api/schema.dbml` draws the schema the migrations build, for dbdiagram.io. After
  adding a migration run `pnpm schema:generate` in `server/` and commit it alongside; the
  API tests fail while it is stale.
- Merging to `main` deploys anything under `server/` to production. You may push it,
  run `server/deploy` scripts against citadel, and set up its config without asking,
  after `agentctl guard` passes. Never print secrets, and ask first before anything
  destructive or irreversible, such as dropping data or deleting accounts.

## Brand assets

- Every logo and icon image lives in `brand/`. Take it from there; never draw or render
  the logo again.
  - `brand/mark/`: the bare S mark in white, black and purple (`#a277ff`), as SVG and
    transparent PNG at 64, 256 and 1024.
  - `brand/icon/`: the mark on the dark app gradient, or the purple "Sikemux Dev" one,
    with rounded or square corners, as SVG and PNG at 256 and 1024.
  - `brand/wordmark/`: the name lettered in Figtree as the site's nav sets it, alone or beside
    the mark, in white, black, or purple mark with white name, as SVG and PNG at 2048 wide.
  - `brand/macos/`: the real Dock icon (Liquid Glass) at 128 to 1024.
  - `brand/social/avatar-400.png`: profile picture.
- The source is `packaging/icons/sikemux.icon`. After changing it, run `pnpm brand` to
  redraw everything; the Dock icon is taken from the installed `/Applications/Sikemux.app`
  (or `SIKEMUX_APP`), so install the new build first.

## Notch

- The island over the MacBook notch is `sikemux-notch`, a SwiftUI helper in `helpers/notch`
  and a client of the core: it reads the device view phones get. The app starts it; it keeps
  running after the app quits.
- It is designed in `helpers/notch/design/screens.html` before it is built, and must keep
  matching it. Change the design in the same commit as the island; view it by serving the repo
  root (`python3 -m http.server`) and opening `/helpers/notch/design/screens.html`.
- Its icons come from `shared/ui/Icons.tsx`, the agent marks and `brand/`. After changing one it
  uses, run `node scripts/generate-notch-icons.mjs`; the release checks fail on a stale copy.
- Its agent rows are laid out like the app's tab rail rows (`app/src/agents/mod.rs`):
  the task with how long ago, the project, then the name, counts and state together at the end.
  When the rail's row changes, change the island's and its design to match.
- `swift run --package-path helpers/notch notch-snapshots <dir>` renders every state to PNGs,
  for comparing with the design.

## Website

- sikemux.com is a separate Astro repo, `nodelike/sikemux-front`, checked out at
  `~/projects/personal/sikemux-front`. Pushing its `main` deploys to Vercel.
- The site reads the version, the download link and the download size from GitHub's
  latest release, both at build time and in the visitor's browser. It finds the
  download by the `_aarch64.dmg` suffix, so renaming that asset or leaving it off a
  release breaks every Download button. Nightlies are pre-releases and never show up.
- The README rounds the download to "about 25 MB" and the site says it is smaller than
  Ghostty (33.8 MB). Update both if a release moves the DMG past either.
- Its screenshots come from this repo. `pnpm shots` in the site runs the phone showcase
  (`mobile/showcase/`) and cuts the feature cards from the Mac pictures already in the
  site, which stay as they are until the GPUI app has a showcase of its own.
- Its copy must stay true of the shipped app and its look follows `DESIGN.md`. When a
  change adds, removes or renames something the site describes, say so, so the site
  can be updated.

## Marketing

- `marketing/` is a separate local Git repo, ignored by this one: `inspiration/` holds
  screenshots of what other products do well, indexed in its README; `outreach/`
  tracks who we contact. File new references and contacts there and commit inside
  `marketing/`, never in this repo.

## Docs

- The user docs for docs.sikemux.com live in `docs-site/`, a Fumadocs site on Astro with
  its own package, dark only in sikemux.com's palette. `pnpm dev` there serves them.
- A change that adds, removes or renames something users see updates its page in
  `docs-site/src/content/docs/` in the same commit.
- The pages under `reference/` are generated: run `pnpm docs:reference` after changing
  a default shortcut, the CLI's help or `browser/tools.json`. CI fails while they are
  out of date.

## UI rules

- Follow `DESIGN.md`. It is short on purpose: keep it to principles and tokens,
  and change it in the same commit when the look moves on.
