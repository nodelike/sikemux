# Contributing to Sikemux

Thanks for taking the time to contribute. Sikemux is a Tauri + Rust + React desktop app; this guide gets you from a clean clone to a green PR.

## Getting started

**Prerequisites:** [Rust](https://www.rust-lang.org/tools/install) (stable), Node.js 22+, and pnpm 10.33.0 (the version pinned in `package.json`). macOS bundles require Xcode; Windows development requires Microsoft C++ Build Tools and WebView2. Published releases target Apple Silicon. Windows is not a current target and CI no longer builds or tests it, so the NSIS installer is best-effort and only as good as the last manual `pnpm build:windows` on a Windows machine.

```bash
git clone git@github.com:nodelike/sikemux.git
cd sikemux
pnpm install        # also points core.hooksPath at .githooks
pnpm tauri dev      # hot-reload Vite + Tauri on macOS or Windows
```

`pnpm install` installs a `pre-push` hook that runs the CI gates for whatever you
are about to push. Install it by hand in an existing clone with `make hooks`.

## Project layout

| Path             | What lives there                                                       |
| ---------------- | ---------------------------------------------------------------------- |
| `src/`           | React UI — components, Zustand state, editor, terminal, themes, keymap |
| `src/api/`       | Thin wrappers over Tauri `invoke` commands                             |
| `src-tauri/src/` | Rust core — PTY, git, LSP, fs watchers, AWS & Rundeck clients          |
| `scripts/`       | Icon pipeline and platform release tooling                             |

## Before you open a PR

The `pre-push` hook already runs these for you. To check without pushing:

```bash
make prepush          # the CI gates, limited to what your commits touched
make check            # every gate, regardless of what changed
pnpm build            # production frontend build
```

`make check` runs Prettier in check mode, ShellCheck, ESLint, TypeScript, frontend tests with `NODE_ENV=test`, `cargo audit`, `cargo fmt --check`, Clippy with warnings denied, Rust tests, and credential-free release-tooling checks. These are the same quality gates enforced by CI.

One CI job stays out of the hook: **macOS launched desktop E2E**. Run it with `pnpm test:e2e:desktop` if you touched the launch path — it builds the whole app, so the hook leaves it to CI.

`cargo audit` reads a database that changes daily, so a push that was clean can go red later with no code change. That is the advisory database moving, not your commit.

Skip the hook for a work-in-progress push with `git push --no-verify`, or `SKIP_PREPUSH=1 git push`. Force every gate to run with `PREPUSH_FULL=1`.

Releases publish only from the Release workflow; `scripts/release.sh --publish` refuses to run outside GitHub Actions. Release tooling supports two explicit modes. The default community mode requires the Tauri updater private key but no Apple membership and produces an updater-signed, ad-hoc code-signed release. `RELEASE_NOTARIZED=1` additionally requires a Developer ID and Apple notarization credentials and enforces Gatekeeper and stapled-ticket verification.

Agent tools are declared once in `browser/tools.json`. Add or change one there, run `pnpm agent-tools:generate`, and commit the regenerated `src-tauri/src/generated_agent_tools.rs`. `pnpm agent-tools:check` fails when a declared method has no handler, when a handler is exposed by no tool, or when the generated file is stale. Keep tool descriptions to one line: they are sent on every agent request, so protocol detail belongs in `browser/SIKEMUX_GUIDE.md`, which agents fetch once through `guide`. `pnpm agent-tools:report` prints what the surface currently costs.

`pnpm perf:budget` checks the production bundle after `pnpm build`. Every startup, lazy-feature, total-JavaScript, and CSS ceiling must retain at least 10% reserve; reaching the nominal ceiling is already a failure. When adding substantial UI, prefer a feature-level dynamic import and only rebaseline a ceiling alongside measured bundle output and a documented reason.

- **Scope** — keep PRs focused. One feature or fix per PR is much easier to review.
- **No regressions** — keep things efficient and performant; if a change touches the editor, terminal, or git panes, verify the affected views still behave.

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

## License

By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE).
