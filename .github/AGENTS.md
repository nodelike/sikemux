# Releasing

The root `AGENTS.md` applies here too.

- Follow `docs/releasing.md`. Releases publish only from the Release workflow, started by
  pushing a `v*` tag; never run `scripts/release.sh --publish` yourself. A stable candidate
  ships by itself: Release starts Ship, which waits out the soak and looks again every
  10 minutes until its gate passes on staff.
- About once an hour, the newest commit of `main` that passed CI becomes a nightly, tagged on that commit.
  The Nightly workflow starts it: never start Release by hand. Stable releases and hotfixes are tagged on
  `release/<major.minor>`, and their version bump and notes go only there.
- Never check out `release/*` in this shared checkout. Commit to it from objects with a
  temporary index, as `docs/releasing.md` shows.
- After a stable `0.x.y`, `main`'s nightlies build toward the next release:
  `0.x.(y+1)-nightly.N`, or `0.(x+1).0-nightly.N` for a bigger one. A hotfix is that next
  release shipped early: fix on `main`, ship a nightly, promote it. Cherry-pick onto
  `release/*` only in an emergency, as `docs/releasing.md` describes.

## The download the site reads

- The site reads the version, the download link and the download size from GitHub's
  latest release, both at build time and in the visitor's browser. It finds the
  download by the `_aarch64.dmg` suffix, so renaming that asset or leaving it off a
  release breaks every Download button. Nightlies are pre-releases and never show up.
- The README rounds the download to "about 30 MB" and the site says it is smaller than
  Ghostty (33.8 MB). Update both if a release moves the DMG past either.
