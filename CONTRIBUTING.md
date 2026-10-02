# Contributing to Sikemux

Thanks for taking the time to contribute. Sikemux is a Tauri + Rust + React desktop app; this guide gets you from a clean clone to a green PR.

## Getting started

**Prerequisites:** [rustup](https://www.rust-lang.org/tools/install), which installs the Rust version `rust-toolchain.toml` pins, the same one CI uses (Homebrew Rust ignores that file), Node.js 22+, and pnpm 10.33.0 (the version pinned in `package.json`). macOS bundles require Xcode; Linux builds require the [Tauri Linux packages](https://v2.tauri.app/start/prerequisites/#linux) (WebKitGTK 4.1, librsvg), and `pnpm build:linux` makes a `.deb`, an `.rpm` and an AppImage; Windows development requires Microsoft C++ Build Tools and WebView2. Published releases target Apple Silicon. Windows is not a current target: the background core that owns terminals and agents is Unix-only, so the app no longer builds there and `pnpm build:windows` does not work.

```bash
git clone git@github.com:nodelike/sikemux.git
cd sikemux
pnpm install        # also points core.hooksPath at .githooks
pnpm tauri dev      # hot-reload Vite + Tauri on macOS or Windows
```

`pnpm install` installs a `pre-push` hook that runs the CI gates for whatever you
are about to push. Install it by hand in an existing clone with `make hooks`.

## Project layout

| Path                  | What lives there                                                                                             |
| --------------------- | ------------------------------------------------------------------------------------------------------------ |
| `src/`                | React UI: components, Zustand state, editor, terminal, themes, keymap                                        |
| `src/state/commands/` | Every action that changes app state, one module per area                                                     |
| `src/api/`            | Thin wrappers over Tauri `invoke` commands                                                                   |
| `src/plugin-api/`     | The only part of the app a plugin's frontend may import                                                      |
| `src/plugins/`        | Plugin frontends (AWS, Bitbucket, Bruno, GitHub, Rundeck, SigNoz)                                            |
| `src-tauri/src/`      | Rust core: PTY, git, LSP, file watchers, browser tabs, agents, diagnostics                                   |
| `src-tauri/crates/`   | Shared Rust crates: the terminal engine, the background core, plugin API, process runner, keychain, markdown |
| `src-tauri/plugins/`  | Plugin backends, one crate per plugin behind a Cargo feature                                                 |
| `browser/`            | Agent tool declarations (`tools.json`) and the guide agents read                                             |
| `showcase/`           | Demo data and scenes for README and site screenshots                                                         |
| `scripts/`            | Code generators, CI checks, icon pipeline and release tooling                                                |
| `docs/architecture/`  | How the app fits together, and the decisions behind it                                                       |
| `mobile/`             | The phone app (Expo) and `@sikemux/native`, the core's client compiled from Rust; its own pnpm workspace     |

Read [the architecture overview](docs/architecture/overview.md) before a large change, and the decision records beside it before proposing to undo one.

### The background core in development

Terminals, agents and tasks run in a separate process, the core (`sikemux core`), which keeps running after the app quits. `make dev` builds the sidecar it runs from. After changing core code, rebuild the sidecar (`pnpm sidecar:dev`) and reload the window: the dev app updates its core in place and your terminals stay open. The dev core listens on `~/.config/sikemux/core.dev.sock` and logs to `core.log` in the dev app's log directory. Quit and Stop Everything (`⌥⌘Q`) stops it. If you touch the launch, quit or update paths, run `pnpm test:e2e:desktop`.

### The mobile app

The phone app lives in `mobile/`, a pnpm workspace of its own, so nothing in it is installed or checked with the desktop app. `mobile/app` is the Expo app and `mobile/native` is `@sikemux/native`, a Turbo Module that `uniffi-bindgen-react-native` generates from `src-tauri/crates/sikemux-mobile`: the core's own client, pairing and iroh connection. [ADR 0008](docs/architecture/0008-mobile-app-expo-and-rust-client.md) records why.

You need rustup's Rust (Homebrew's Rust ignores `rust-toolchain.toml` and has no phone targets), Xcode, and for Android the NDK and `cargo-ndk`. `rust-toolchain.toml` lists the phone targets, so rustup installs them with the toolchain:

```bash
cargo install cargo-ndk     # Android only
cd mobile
pnpm install
pnpm run doctor             # checks the toolchain, Xcode, the Android SDK and what is built
pnpm native:ios:sim         # builds the Rust client for the simulator and generates its bindings
pnpm ios                    # builds the development app and runs it in the simulator
```

Rebuild with `pnpm native:ios:sim` after changing `sikemux-mobile` or the protocol in `sikemux-core`; it builds only the simulator slice, which is the quickest loop. `pnpm native:ios` adds the device slice, `pnpm native:android` builds for Android, and the `:release` variants build with the small `mobile` profile that release runs require. These scripts put `~/.cargo/bin` first on `PATH` themselves and stop if the toolchain or a target is missing. `pnpm clean` deletes the generated native projects and bindings. The generated bindings and native libraries are build output and are not committed. The app needs a development build; Expo Go cannot load `@sikemux/native`. To pair the simulator with your Mac, turn on Settings, Devices in a dev build of Sikemux.

`pnpm check` in `mobile/` runs the phone's typecheck, lint, Prettier check and unit tests (vitest, with stand-ins for React Native and the native modules in `mobile/app/test/mocks`). Its lint also fails if Mac code the phone imports through `@mac/` reaches `@tauri-apps/*`, which Metro cannot bundle; the repo's `pnpm lint` runs that check too.

#### Designing phone screens

Every phone screen is drawn first in `mobile/design/screens.src.html`, with the Mac app's own icons and fonts. A design change starts there, before the app: run `pnpm design` in `mobile/` and open http://127.0.0.1:8791/mobile/design/screens.html, which rebuilds on each reload. In the PR, add a screenshot of each screen you changed or added from that page, so the design is reviewed before the code. `{{IconName}}` or `{{IconName:size}}` in the file draws one of the app's icons. The glyphs the phone draws itself live in `mobile/app/src/ui/drawnIcons.ts`, which the app and the page both read.

## Before you open a PR

The `pre-push` hook already runs these for you. To check without pushing:

```bash
make prepush          # the CI gates, limited to what your commits touched
make check            # every gate, regardless of what changed
pnpm build            # production frontend build
```

`make check` runs Prettier in check mode, ShellCheck, ESLint, TypeScript, frontend tests with coverage, the performance budget, `cargo audit`, `cargo fmt --check`, Clippy with warnings denied, Rust tests, and credential-free release-tooling checks. It also checks that generated files (IPC contracts, agent tools, grammars) match their sources. These are the same quality gates enforced by CI.

Some gates are floors that only move one way. Raise them when you beat them; never lower them to get a change through:

- **Test coverage:** the thresholds in `vite.config.ts` under `test.coverage.thresholds`. When a change lifts coverage, set each threshold to the new measured value, rounded down.
- **`!important` in CSS:** the budget in `scripts/check-css-important.mjs`, checked by `pnpm lint`. Remove one and lower the budget to match.
- **Bundle size:** the ceilings checked by `pnpm perf:budget` (see below).

One CI job stays out of the hook: **macOS launched desktop E2E**. Run it with `pnpm test:e2e:desktop` if you touched the launch path — it builds the whole app, so the hook leaves it to CI.

`cargo audit` reads a database that changes daily, so a push that was clean can go red later with no code change. That is the advisory database moving, not your commit.

Skip the hook for a work-in-progress push with `git push --no-verify`, or `SKIP_PREPUSH=1 git push`. Force every gate to run with `PREPUSH_FULL=1`.

Releases publish only from the Release workflow ([docs/releasing.md](docs/releasing.md)); `scripts/release.sh --publish` refuses to run outside GitHub Actions. Release tooling supports two explicit modes. The default community mode requires the Tauri updater private key but no Apple membership and produces an updater-signed, ad-hoc code-signed release. `RELEASE_NOTARIZED=1` additionally requires a Developer ID and Apple notarization credentials and enforces Gatekeeper and stapled-ticket verification.

Agent tools are declared once in `browser/tools.json`. Add or change one there, run `pnpm agent-tools:generate`, and commit the regenerated `src-tauri/crates/sikemux-core/src/cli/methods.rs`. `pnpm agent-tools:check` fails when a declared method has no handler, when a handler is exposed by no tool, or when the generated file is stale. Keep tool descriptions to one line: they are sent on every agent request, so protocol detail belongs in `browser/SIKEMUX_GUIDE.md`, which agents fetch once through `guide`. `pnpm agent-tools:report` prints what the surface currently costs.

`pnpm perf:budget` checks the production bundle after `pnpm build`. Every startup, lazy-feature, total-JavaScript, and CSS ceiling must retain at least 10% reserve; reaching the nominal ceiling is already a failure. When adding substantial UI, prefer a feature-level dynamic import and only rebaseline a ceiling alongside measured bundle output and a documented reason.

- **Scope** — keep PRs focused. One feature or fix per PR is much easier to review.
- **No regressions** — keep things efficient and performant; if a change touches the editor, terminal, or git panes, verify the affected views still behave.

## Screenshots

`pnpm showcase` renders the app in headless Chrome on demo data and writes every scene to `showcase/out/` as a full window, a framed window, and 2x crops. It needs Google Chrome installed. Add `--publish` to refresh the README images, or `--site <dir>` to copy every capture somewhere else. Scenes live in `showcase/scenes.mjs` and the demo data in `showcase/world/`. To click around the same demo app yourself, run `make showcase`; it opens http://localhost:1471/showcase/ in your browser.

## Commit messages

Follow [Conventional Commits](https://www.conventionalcommits.org/) — the format already used in this repo:

```
feat(themes): add custom theme editor
fix(sessions): treat agent view as project-only
style(bruno): tighten sidebar row styling
chore(release): v0.1.20
```

Common types: `feat`, `fix`, `style`, `refactor`, `perf`, `chore`, `docs`. Scopes match the area you touched (`editor`, `git`, `aws`, `rundeck`, `bruno`, `terminal`, `themes`, …).

## Reporting bugs & requesting features

Open an [issue](https://github.com/nodelike/sikemux/issues) with:

- What you expected vs. what happened, and steps to reproduce.
- Your operating-system version and the Sikemux version (shown in the side rail).
- Logs or screenshots where relevant.

For questions, or to talk an idea through before opening an issue, ask in the [Sikemux Discord](https://discord.gg/UKfmHpF9kX).

## License

Before your first pull request can be merged, you sign the [Contributor License Agreement](CLA.md). A bot comments on the pull request with the one line to post. You keep the copyright in your work; the agreement lets nodelike license it under the project's [license](LICENSE) and under other terms, including commercial ones.
