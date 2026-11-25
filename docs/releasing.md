# Releasing

Releases publish from the **Release** GitHub Actions workflow, never from a laptop. Nightlies build themselves from `main` at most once an hour (see [Version numbers](#version-numbers)). A stable release needs the version bump and a `RELEASE_NOTES.md` headed `# Sikemux v<version>` committed on its `release/<major.minor>` branch, then the matching tag pushed. Only the owner can push a `v*` tag. The tag builds a candidate that only the staff feed serves, and the **Ship** workflow makes it stable by itself once it has soaked there (see [Shipping a stable release](#shipping-a-stable-release)):

```bash
make build
git tag v0.4.1 && git push origin v0.4.1
```

Pushes run only rustfmt and the commit-credit check locally (`.githooks/pre-push`); CI checks `main` one run at a time, and when pushes arrive faster than it runs only the newest waiting one runs. Tag a commit whose CI run is green or running: the release waits up to two hours for that run, and runs the checks itself when the commit has none, its run failed or it is still not done. `make build` builds the DMG against its size limit, which otherwise surfaces only in the Release run.

A run is titled with its tag, or with the commit for a nightly, and it stops if the tag disagrees with `package.json` or the release already exists. The workflow reads the version from `package.json` and builds, signs and verifies the Mac release while the full CI suite runs. No job that runs the build's dependencies holds signing material, and no job that holds it runs them or anything they built:

1. **Build the helpers** compiles the voice and simulator helpers (`scripts/release.sh --stage helpers`).
2. **Sign the helpers** signs them with the Developer ID (`scripts/sign-helper.sh`). The app is compiled with their SHA-256 and refuses any other download, so they are signed before it is built.
3. **Build the app** runs once the signed helpers are ready and compiles the app around them (`scripts/release.sh --stage compile`), while **Build the Mac core** compiles the core, which does not embed their hashes. Once the core arrives (`scripts/wait-for-artifact.sh`), the same job builds the bundle and the DMG around both, ad hoc signed (`scripts/release.sh --stage package`, with the core handed over as `PREBUILT_CORE`).
4. **Sign and notarize the app** signs the app on a fresh machine with Apple's tools and a Tauri CLI pinned by hash, notarizes it, packs and signs the updater archive, and puts the signed app back into the DMG (`scripts/sign-mac.sh`).
5. **Verify the signed release** checks every signature, notarization ticket, hash, architecture and size, starts the signed CLI and helpers, and writes `latest.json` (`scripts/verify-release.sh`).

Beside it, `pnpm build:linux` (`scripts/build-linux.sh`) builds the GPUI app as the `.deb` and `.AppImage` for the machine it runs on, x86_64 and arm64, and the `Sikemux_<version>_<amd64|arm64>.tar.gz` the x86_64 Arch package is made from. The Release workflow and CI build them in an `ubuntu:22.04` container, so they run on its glibc 2.35 and newer (`OLDEST_APP_GLIBC` in `scripts/install.sh`), and `scripts/check-glibc.sh app` fails the build, naming the symbol, if any program or library in them asks for a newer one. The job compiles the app (`BUILD_STEP=compile`) while **Build the core of the Linux packages** builds their core on a machine of the same kind, then packs the app around that core (`BUILD_STEP=package`, with the core handed over as `PREBUILT_CORE`). The signing job signs both AppImages with the updater key, and the publish step adds them to the update feed as `linux-x86_64` and `linux-aarch64`, so an AppImage updates itself; the `.deb` updates through Sikemux's package repository (see [Package repositories](#package-repositories)), and the Arch package through pacman or the installer, whose command the app shows. Each Linux host core joins the feed the same way, as `host-linux-x86_64` and `host-linux-aarch64`, which a host's root update service reads. Once the builds and the checks pass, `scripts/publish-release.sh` publishes them together. A `-nightly.N` version goes to the staff channel at once and to nightly after a soak, and a plain version to the staff channel as a stable candidate; the workflow refuses any other prerelease. Before publishing, **Rehearse the update** (`.github/workflows/update-rehearsal.yml`) installs the latest stable host core with its own installer on Ubuntu, x86_64 and arm64, and has it update itself through its own root updater to the host cores this run signed, served from a stand-in github.com on the runner (`scripts/rehearse-host-update.sh`). The Mac app and the AppImage install an update only from their own window, so the staff machines that take each build first rehearse those, and report every update to Sentry. Only one nightly runs at a time, and each run keeps its built artifacts.

**Build the Arch package** makes `sikemux-bin-<version>-1-x86_64.pkg.tar.zst`, or `sikemux-nightly-bin-…` for a nightly, from the amd64 tarball in an `archlinux:latest` container, which has no arm64 image, so arm64 has no Arch package and the installer gives Arch on arm64 the AppImage (`packaging/arch/build-release.sh`). `packaging/arch/prepare.sh` writes the release's PKGBUILD from `packaging/arch/PKGBUILD`, pinned to the tarball's checksum, with its `.SRCINFO`. pacman sorts a version with its hyphen dropped, `0.6.1nightly.6`, below `0.6.1`, so a stable release overtakes its nightlies. Arch packages must hold only root-owned files, so the core is made setgid `sikemux` by `usr/lib/tmpfiles.d/sikemux.conf` after every install and upgrade, once `usr/lib/sysusers.d/sikemux.conf` has made the group. The job lints the package with namcap, installs it, upgrades it to a rebuild, and after each checks the core is setgid `root:sikemux`, `/var/lib/sikemux` is `770 root:sikemux`, every library the app needs is there and `sikemux-host --version` runs. The release publishes the tarball and the package. The job also writes `sikemux.db`, the index of a pacman repository holding just that package, and checks pacman lists the package from it.

The latest stable release is that pacman repository: pacman reads `sikemux.db` and the package from `releases/latest/download`, so it moves to each stable once Ship makes it latest, with nothing to update. On x86_64 Arch, the stable installer adds it to `/etc/pacman.conf` as `[sikemux]`, with `SigLevel = Required DatabaseRequired`, after trusting the archive key the release carries as `sikemux-archive.asc` (`pacman-key --add`, then `--lsign-key` with the key's fingerprint, which the installer pins). From then on `pacman -Syu` updates Sikemux with the rest of the system, and the core offers `sudo pacman -Syu` in place of the installer. The installer adds it only when the release carries `sikemux.db`.

Every release also publishes `scripts/install.sh` as `install.sh`, the installer made for its own files, and sikemux.com/install serves the latest stable release's copy. A change to the installer therefore reaches people only with a release, never with a push to `main`. The host job builds each core on AlmaLinux 9, whose glibc 2.34 is the oldest a host may have (`OLDEST_GLIBC` in `scripts/install.sh`, which refuses older systems), and `scripts/package-host.sh` fails, naming the symbol, if the core asks for a newer glibc than that. Before publishing, the host job installs the core it just built with that installer, twice, on that same AlmaLinux 9, from the files laid out as the release will have them (`scripts/smoke-host-install.sh`), and stops the release if either install fails. With `SIKEMUX_CHANNEL=nightly`, the stable installer hands over to the nightly's own `install.sh`. A separate job then signs each host core with the updater key (`scripts/sign-host.sh`), running nothing but a pinned Tauri CLI, and the release carries each signature beside its core as `.tar.gz.sig`: a host updating itself installs only a core that signature covers.

The Mac app and core ship stripped. Their release builds first move the symbols to a dSYM, which the release publishes beside the DMG as `Sikemux_<version>_aarch64.app.dSYM.zip` and `.core.dSYM.zip`. It names every function, and the file and line of every frame in Sikemux's own crates; a dependency's frames get no line, since every crate's line tables carried through the link's LTO need more memory than a runner has. A panic in any Sikemux process writes `~/.config/sikemux/autopsy/panic-*.json` with its message and line, and `sikemux doctor` lists the latest. `scripts/symbolicate.sh <report>` takes that file or a macOS crash report (`~/Library/Logs/DiagnosticReports/sikemux-*.ips`), downloads the matching dSYM and names every frame.

If a release fails before it publishes, fix it and move the tag onto the fix. Only the owner can move a release tag, and moving it starts a fresh run:

```bash
git tag -f v0.4.1 && git push -f origin v0.4.1
```

Every release takes its signing material from the `release` environment:

| Name                                                                                                                                       | Kind                    | Needed for         |
| ------------------------------------------------------------------------------------------------------------------------------------------ | ----------------------- | ------------------ |
| `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`                                                                          | secret                  | every release      |
| `RELEASE_NOTARIZED`                                                                                                                        | variable, `1` to enable | notarized releases |
| `APPLE_CERTIFICATE` (base64 `.p12`), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | secrets                 | notarized releases |
| `HOMEBREW_TAP_SSH_KEY`                                                                                                                     | secret                  | the Homebrew cask  |
| `PACKAGES_SIGNING_KEY` (armored signing subkey), `PACKAGES_SIGNING_KEY_PASSWORD`                                                           | secrets                 | every release      |

In this workflow only the three signing jobs use the `release` environment, which holds the secrets and has no reviewer, and in Ship only its gate and the Homebrew update. The Bitbucket, Jira and Vercel OAuth client secrets and the Klipy API key are compiled into the core, so they are repository secrets the jobs that build a core read. A stable tag builds and publishes its candidate without asking; only shipping it waits on the owner. A nightly publishes as soon as its checks pass.

After a stable release publishes, **Update the Homebrew cask** downloads its DMG and pushes `Casks/sikemux.rb`, pinned to that DMG's checksum, to the `nodelike/homebrew-tap` repository (`packaging/homebrew/push-cask.sh`), so `brew install --cask nodelike/tap/sikemux` installs it. It pushes with a write deploy key on `nodelike/homebrew-tap`, stored as `HOMEBREW_TAP_SSH_KEY` in the `release` environment, and skips with a note in the run's summary while that is unset. Nightlies never go to Homebrew; the app updates itself either way, so the cask says `auto_updates`.

Run `scripts/release.sh` locally without `--publish` to preview a release: it runs the same steps in order, building, signing and verifying everything without touching GitHub. The build steps never see the signing material in their environment, but they run on a Mac whose keychain holds the Developer ID, so only the workflow keeps the two apart.

## Package repositories

The `.deb` adds a repository for its channel when installed (`packaging/linux/repository.sh`, joined to its install script by `scripts/build-linux.sh`), and removes it when removed (`packaging/linux/postrm.sh`). It names the channel's suite, `packages-stable` or `packages-nightly`, and the key the package installs beside it. apt reads the suite from the fixed GitHub release of that name. Its index names each `.deb` by its own release's folder (`v<version>/…`), so the packages stay where every release already puts them.

**Sign the Linux host cores and the AppImages** also runs `scripts/sign-packages.sh`. It writes the apt index once per suite the version may serve: nightly, and stable as well for a stable version, because apt stops when a suite's `Suite` changes. Each index comes as `Packages.xz` and `Packages.gz`, each with its own `InRelease`, `Release` and armored `Release.gpg`. Every signature is checked against the key before the job ends, and the release carries the indexes as `packages-index-<version>.tar`. It also signs `sikemux.db` and the Arch package with the same key, as binary detached `sikemux.db.sig` and `<package>.sig`, which the release carries beside them; with no signature inside the index, pacman fetches the package's from beside it.

A suite moves in the **Package repository** workflow (`.github/workflows/packages.yml`), one run per suite at a time. A stable release starts it for both suites, and moving the nightly feed starts it for nightly. Its move job (`scripts/move-package-suite.sh`) reads the target from the feeds, not from whoever started it, so the one run GitHub keeps when several wait still makes every move: stable follows the latest stable release, and nightly follows the nightly feed, or stable when stable is newer. A release without an index leaves the suite as it is. The move copies that release's index into the suite's release, signing nothing.

No apt client sees an index that disagrees with the signed file naming it. Each move switches the suite between `Packages.xz` and `Packages.gz`, so the file the previous `InRelease` names stays up. The new index goes up first. Then the new `InRelease` goes up as `InRelease.next`, the old one is deleted, and the new one is renamed into place. Only between those last two API calls, well under a second, is there no `InRelease`, and then apt reads `Release` and `Release.gpg`, which still name the previous index. Those two are replaced the same way, after the new `InRelease` is up. The previous index is deleted 60 seconds later, more than an apt that read the previous `InRelease` needs to fetch it.

The key was made once with `scripts/make-packages-key.sh`, which prints nothing secret. Its RSA 4096 primary key only certifies, and stays with its revocation certificate in `~/.config/sikemux/release/packages-signing-{primary,revocation}.asc` to back up off the Mac. Its passphrase is in the Keychain entry "Sikemux packages signing key". Only the signing subkey, `packages-signing-subkey.asc`, goes to the `release` environment, and it expires two years after it was made. Before then, extend it with the primary key, re-export the public key to `packaging/linux/sikemux-archive.asc` and the subkey to `PACKAGES_SIGNING_KEY`, and release, so every package installs the extended key. The public key is also fetched by hand from `packaging/linux/sikemux-archive.asc` on `main`.

## Omarchy

Omarchy installs Sikemux from its own package repository, pkgs.omarchy.org, not from the AUR. `packaging/omarchy/` holds the two upstream changes that put it there, made once and submitted in this order:

1. `omarchy-pkgs/pkgbuilds/sikemux-bin/`, a pull request to omacom/omarchy-pkgs. It is `packaging/arch/PKGBUILD` made to that repository's rules, for x86_64 from the amd64 tarball and aarch64 from the arm64 one: each tarball's checksum is pinned, the tarball is unpacked with `bsdtar --no-same-owner` so every file is root-owned, and `.omarchy/package.json` watches this repository's GitHub releases for stable `vX.Y.Z` tags. Set `pkgver` to a stable release that carries the tarball and run `updpkgsums` before submitting; the all-zero checksums stand in until then. Once it merges, their bot opens a pull request for each stable release and merges it when it builds, so a release needs nothing from us. Its `sikemux.sysusers` and `sikemux.tmpfiles` are copies of `packaging/arch/`'s: a change to either goes to omarchy-pkgs as a pull request with `pkgrel` bumped.
2. `omarchy/0001-Add-Sikemux-to-Install-AI.patch`, a pull request to omacom/omarchy once `sikemux-bin` is listed on pkgs.omarchy.org. It adds Sikemux to _Install > AI_ and _Remove > AI_, its mark to the Omarchy icon font, and `omarchy-remove-ai-sikemux`, which stops the core and its user service before dropping the package. That script names the service, `~/.local/share/com.nodelike.sikemux`, `~/.config/sikemux` and `/var/lib/sikemux/<uid>`, so moving any of them means a pull request there too. The install row shows on x86_64 and aarch64 alike, since Omarchy asks that its menu match across platforms.

## Community releases without an Apple Developer membership

The updater and Apple Gatekeeper trust different signatures. By default, `scripts/release.sh` makes a community release. It signs the updater archive with the Tauri updater key and applies an ad hoc code signature to the app and DMG.

Existing community installations can receive in-app updates. Fresh downloads are not notarized by Apple, so macOS may ask you to remove quarantine again. Keep the updater private key secure. Clients reject archives that do not match the public key bundled with the app. They also reject a file whose signed comment does not name that file and the version offered, a version no newer than the one running, and a nightly on the stable channel (`crates/sikemux-release`). A feed is not signed, so nothing it claims counts until the comment says the same.

Every release is a versioned GitHub release holding the build and its `latest.json`. The default channel follows the latest stable release's. A nightly requires a `-nightly.N` version and is published as a prerelease. Two moving releases hold only a `latest.json` each and point at one of these builds: `staff`, which the staff channel follows, and `nightly`, which the opt-in Nightly channel follows. `scripts/move-feed.sh` points one at a build by copying that build's `latest.json`, so moving a feed rebuilds nothing.

Stable is cut from a `release/<major.minor>` branch and nightly from `main`. A nightly targets whichever version comes next, whether that is a patch, a minor or a major, and a stable release of that version overtakes its nightlies for nightly users too.

## Version numbers

About once an hour, the newest commit of `main` that passed CI becomes a nightly, with no bump commit. The **Nightly** workflow runs each time CI passes on `main`, and at seven past each hour in case GitHub skips that, finds that commit, and starts Release on it when it is newer than the last nightly and the one still building; otherwise it does nothing. `scripts/stamp-nightly.sh` gives that commit its version and notes in the run, never on `main`: the next stable version, then `-nightly.` and the number of commits since the last stable release left `main`, such as `0.6.1-nightly.2486`. A later commit always has a higher number. The next stable version is the one in `main`'s `package.json`, or the first patch after it not yet tagged stable. The notes list what each commit since the previous nightly changed in the app, by `scripts/release-notes.mjs`: its `Docs:` lines, or its subject when it has none. A commit that touches only CI, scripts, docs, tests or the server without a `Docs:` line is left out, and one marked `Docs: none` too. A `Nightly:` line is listed in a nightly's notes alone, so a stable's notes, drafted with `node scripts/release-notes.mjs app <last stable> <nightly>`, leave it out. When nothing is left, the notes say so. Mobile CI, Server and Docs do not hold a nightly back.

Only one nightly builds at a time. A newer one cancels it, but Nightly lets a build finish until its current attempt has run 90 minutes, so a busy `main` still publishes about once an hour and never cuts off a rerun that is about to finish. A commit whose CI failed never gets a nightly; the next green one ships what it held. When a nightly Release or `main`'s newest push CI run fails only because GitHub never assigned a runner (or a job waiting on an artifact from one), Nightly reruns only its failed jobs up to three times so finished builds stay warm; real failures stay red and Nightly's next run retries a nightly from scratch as before, so nobody starts Release by hand and the gate refuses `gh workflow run release.yml` from an agent. The publish step makes the `v<version>` tag itself: the "Protect nightly tags" ruleset lets Release create `v*-nightly.*` tags, while only the owner can move or delete them, and only the owner can create any other `v*` tag. GitHub refuses that tag on a commit whose workflow files no branch has, so Release first moves the `nightly-build` branch to the commit, and a workflow change on `main` during the build cannot stop the tag. Mobile release does the same with `mobile-nightly-build`. A tag made by the workflow starts no other workflow. The owner can still push a nightly tag by hand.

A nightly links with thin LTO, and its Mac jobs compile through sccache, kept in the repository's Actions cache, so it builds in about half the time of a stable release, which keeps fat LTO and no cache, for a slightly larger DMG.

A nightly's plan also starts the **Windows download** workflow on the same commit and version, which builds the Windows installers beside the Mac's jobs but outside the Release run, so the next nightly never waits for Windows. Once Release has published the nightly, its **Add the installers to the release** job signs them with the updater key (`scripts/sign-host.sh <version> windows`), attaches `Sikemux_<version>_<x64|arm64>-setup.exe` to the release, adds them to its `latest.json` as `windows-x86_64` and `windows-aarch64`, and repoints any feed already serving that nightly (`scripts/publish-windows.sh`). A failed Windows build leaves the nightly as it is, and a Release that ends without publishing cancels its Windows run. A stable release gets its installers by starting Windows download by hand with its version, its tag as `ref`, and its Release run's id as `release_run`; the stable channel then reads them from that release's `latest.json`.

A published nightly reaches the `staff` feed at once. Release then starts **Nightly soak**, which waits 30 minutes while it runs on staff machines and then moves the `nightly` feed to it, unless a newer build is already there or it was pulled. On the owner's Mac:

- `scripts/nightly.sh why` says what each feed serves and whether nightlies are held.
- `scripts/nightly.sh hold "<reason>"` stops Nightly from starting builds, through the `NIGHTLY_HOLD` repository variable, and `scripts/nightly.sh unhold` lets them go again.
- `scripts/nightly.sh pull <version>` marks a published build pulled, so the nightly feed never takes it, and moves any feed that serves it back to the newest build before it that was not pulled. Updaters install only newer versions, so a machine that already runs the pulled build keeps it until the next nightly ships. Fix forward.

CI on `main` never cancels a run that has started, so each run that starts gives a verdict, and of the pushes that arrive while it runs only the newest is checked next. Only Clippy on macOS and the GPUI app's tests take a Mac; the rest of a run is on Linux, since GitHub's macOS runners are scarce and a job no Mac takes within about 15 minutes is cancelled. Checking every push would still queue runs behind each other and delay the newest verdict, which is the one a nightly needs.

A nightly is a prerelease of the next stable release. After a stable `0.x.y`, `main`'s nightlies are `0.x.(y+1)-nightly.N` by themselves, or `0.(x+1).0-nightly.N` once `main`'s `package.json` is set to `0.(x+1).0` for a bigger one. Stable releases come every few days, each a promoted nightly.

## Promoting a nightly to stable

A nightly that has been in use becomes stable without anything newer from `main`. On `release/<major.minor>`:

1. A merge commit whose content is exactly that nightly's tagged commit.
2. A commit that sets the stable version in `package.json`, `[workspace.package]` in `Cargo.toml` and the workspace's own crates in `Cargo.lock`, and writes `RELEASE_NOTES.md` for everything since the last stable release.
3. The `v<version>` tag on that commit.

None of this lands on `main`. Commits made to `main` meanwhile stay out of the release.

## Shipping a stable release

A stable tag's Release run publishes its build as a prerelease and points the staff feed at it, so staff machines run the files that will ship. It makes no latest release, moves no package suite, and leaves Homebrew alone. While a candidate builds and while it waits, the staff feed keeps to it: new nightlies still publish and soak, but leave the staff feed alone, which also never moves back to an older build. A nightly of the next release that reached staff first would keep staff Macs from ever updating to the candidate, so nobody needs to hold nightlies around a stable tag.

**Ship** (`.github/workflows/ship.yml`) starts when Release publishes the candidate and waits until it has been on staff for 30 minutes. While the candidate is still short of its Macs, the run starts the next one 10 minutes later, until it ships or the gate goes red; GitHub's cron runs too seldom to rely on. `gh workflow run ship.yml -f version=0.6.1` starts the chain again by hand, and with no version it takes the candidate the staff feed serves. Its gate works the evidence out itself (`scripts/ship-gate.sh`): the candidate is still a prerelease and not pulled, the staff feed serves it, its Release run passed, it has been out for 30 minutes, at least 2 Macs updated to it, or fewer when fewer follow the staff feed: as many Macs as updated on the staff channel to the most taken of the five nightlies published before it, and never fewer than one, and Sentry holds no crash from it and no failed or rolled back update to it, read with `SENTRY_READ_TOKEN`, a token with only the `event:read` and `org:read` scopes. It writes that on the run's page, with the notes. While the candidate is still short of its Macs, the run ends green and the next one looks again. If anything else fails, the run goes red and nothing ships. Once it all passes, `scripts/ship-release.sh`, with nobody asked, marks the candidate the latest release, which is the stable feed, the installer and the site's download, and moves both package suites, and then the cask and the AUR follow. Nothing is built, so that takes about a minute.

To reject a candidate, `scripts/nightly.sh pull <version>` marks it pulled and moves the staff feed back. Its version is spent, and the next candidate takes the next patch.

## Hotfixes

A hotfix is the next release, shipped early. Fix the bug on `main`, ship a nightly, and promote that nightly to stable. It takes the version the nightlies were already building toward, so nothing collides, and it carries whatever else is on `main`.

Cherry-pick onto `release/<major.minor>` only when `main` holds something too risky to ship, such as a half-finished refactor during a security fix. The cherry-picked hotfix then takes the version the nightlies are building toward, and the updater takes the newest version across both channels, so first bump `main` to the patch after it and publish that nightly. Then cherry-pick only the fix onto the release branch, bump to the claimed patch, write short notes and tag it. Never merge a release branch into `main`, or `main` into a release branch.

## Committing to a release branch from the shared checkout

Several agents work in one checkout at once, so never check out `release/*`, stash, or use a worktree. Build the commits from objects with a temporary index and push them by id. Use `${name}:path`, not `$name:path`: zsh reads `:s`, `:h` and the like after a bare variable as modifiers.

Promote a nightly:

```bash
release=release/0.4 nightly=v0.4.3-nightly.2486 version=0.4.3
git fetch origin
base=$(git rev-parse "origin/${release}"); src=$(git rev-parse "${nightly}^{commit}")
merge=$(git commit-tree "${src}^{tree}" -p "${base}" -p "${src}" -m "merge: bring ${nightly} onto the ${release#release/} line for v${version}")
export GIT_INDEX_FILE=$(mktemp -u); git read-tree "${merge}"
from=$(git show "${merge}:package.json" | sed -n 's/.*"version": "\(.*\)".*/\1/p')
put() { git update-index --cacheinfo "100644,$(git hash-object -w --stdin),$1"; }
git show "${merge}:package.json" | sed "s/\"version\": \"${from}\"/\"version\": \"${version}\"/" | put package.json
git show "${merge}:Cargo.toml" | sed "s/^version = \"${from}\"$/version = \"${version}\"/" | put Cargo.toml
# Only the workspace's crates: a crates.io package can share the old version, and it has a source line.
git show "${merge}:Cargo.lock" | awk -v from="${from}" -v to="${version}" '
  held != "" { if ($0 !~ /^source = /) sub("\"" from "\"", "\"" to "\"", held); print held; held = "" }
  $0 == "version = \"" from "\"" { held = $0; next }
  { print }' | put Cargo.lock
put RELEASE_NOTES.md < /path/to/notes.md
bump=$(git commit-tree "$(git write-tree)" -p "${merge}" -m "chore(release): prepare v${version}")
rm -f "${GIT_INDEX_FILE}"; unset GIT_INDEX_FILE
git diff --stat "${merge}" "${bump}"   # only the version fields and the notes
git push origin "${bump}:refs/heads/${release}"
git tag "v${version}" "${bump}" && git push origin "v${version}"
```

Cherry-pick a fix, then bump and tag the same way with the patch version:

```bash
release=release/0.4 fix=<commit on main>
base=$(git rev-parse "origin/${release}")
out=$(git merge-tree --write-tree --merge-base="${fix}^" "${base}" "${fix}") || { echo "${out}"; echo "conflict: fix it on the release branch by hand"; }
picked=$(git commit-tree "${out%%$'\n'*}" -p "${base}" -m "$(git log -1 --format=%B "${fix}")")
```

The push to `release/*` fails unless it fast-forwards, and only the owner may push to these branches or push a `v*` tag.

## Running the release script

```bash
./scripts/release.sh 0.3.5 "Release notes"
./scripts/release.sh 0.4.0-nightly.1 "Nightly notes" --nightly
```

If you have an Apple Developer membership, set `RELEASE_NOTARIZED=1` with the Developer ID and notarization environment variables. The release script then requires a successful Gatekeeper assessment and stapled notarization tickets, and fails without them.

## Releasing the phone app

The phone app releases from the **Mobile release** workflow. As on the Mac, the jobs that install and build the dependencies hold no signing key, and the jobs on the `release` environment that sign and upload never run them or anything they built.

A nightly is cut by itself each time CI passes on a push to `main` that changed what the phones build from (`mobile/`, `shared/`, and the `sikemux-mobile`, `sikemux-wire` and `sikemux-client` crates) since the last phone release. One builds at a time, and of the pushes waiting only the newest gets the next run; `scripts/nightly.sh hold` holds these too. `mobile/scripts/plan-release.sh` gives it the next `-nightly.N` of `app.json`'s version, or of the patch after it once that version shipped stable, and notes by the same rule from the commits that touched the phone, all written in the run, never on `main`. The publish step makes its `mobile-v*-nightly.*` tag. Nightlies go to testers: the closed test on Play's alpha track, and TestFlight's internal group, which needs no review.

A stable release starts from a `mobile-v<version>` tag, which only the owner can push, on a commit whose phone code a nightly of that version was built from; only `mobile/RELEASE_NOTES.md`, the stable's notes, may differ. Nothing is rebuilt for the iPhone: the build that nightly sent to TestFlight goes up for App Store review. Android is rebuilt from the same code for stable's update channel.

```bash
git tag mobile-v0.2.1 && git push origin mobile-v0.2.1
```

`mobile/app/app.json` and `MARKETING_VERSION` in `mobile/ios/project.yml` hold the same plain version, such as `0.1.0`, because iOS refuses anything else. A release is either that version, which is stable, or that version with `-nightly.N`. The run stops if a tag is not a release of the version in `app.json`, or if the two files disagree.

Android needs a version code that grows with every upload. `versionCode` in `app.config.js` derives it from the version: `0.5.0-nightly.3` is `500003`, and `0.5.0` itself is `500999`, so a stable build always follows its own nightlies. Minor and patch numbers stay below 100, and nightlies below 999. An iPhone stable keeps its nightly's build number, so the app counts itself as the stable when the App Store installed it. The iOS build number is the same code, and the app reads its release back from it to compare with the oldest version `GET /v1/network` allows.

The run checks the phone app, builds the Rust client with the small `mobile` profile, and builds the APK and the Play app bundle unsigned. A separate job signs them with the Play upload key, using only the Android SDK's `zipalign` and `apksigner` and the JDK's `jarsigner`, and refuses either unless it carries the upload key's certificate. A third job publishes them. It attaches the APK to a GitHub release of the tag, marked a prerelease for a nightly, and never as the latest release: sikemux.com takes its Mac download from that one. It also attaches `runtime-android.txt`, the runtime version the build was made for (see below). Last, it puts the app bundle on Google Play: a nightly on the closed test's alpha track, and a stable release on production, where it starts with 10% of phones (the app's first production release reaches every phone at once, since Google Play stages only updates). Once it holds up, widen it with `node scripts/play-release.mjs rollout 0.5` in `mobile/app`, then `rollout 1` for every phone, with the service account's key in `PLAY_SERVICE_ACCOUNT`. Google Play keeps production closed until the closed test has had 12 testers for 14 days, so a stable tag fails at that step until then.

Every phone release needs its notes in `mobile/RELEASE_NOTES.md`, committed before the tag: a `# <version>` heading, then what changed in plain text, at most 500 characters. Google Play shows them as "What's new" and the GitHub release opens with them. The run checks the file before it builds and stops if the heading is for another version, so `node scripts/play-release.mjs notes <version>` in `mobile/app` shows what Google Play will get.

| Name                      | Kind   | Holds                                                                                         |
| ------------------------- | ------ | --------------------------------------------------------------------------------------------- |
| `ANDROID_UPLOAD_KEYSTORE` | secret | the upload keystore, base64                                                                   |
| `ANDROID_UPLOAD_PASSWORD` | secret | its password, which is also the key's own password                                            |
| `PLAY_SERVICE_ACCOUNT`    | secret | the `sikemux-play-release` service account's JSON key, allowed to release in the Play Console |

The same run builds the native iPhone app (`mobile/ios`) on a macOS runner and sends it to TestFlight, as build number the version code above, and with the short commit in its `SikemuxCommit` Info.plist key for the account sheet. The build job builds the Rust client for the device with the `mobile` profile (`mobile/ios/scripts/build-core.sh --profile mobile --device-only`), generates the Xcode project with XcodeGen and archives the Release configuration unsigned, then signs the archive ad hoc with each bundle's entitlements (`mobile/ios/scripts/entitlements.mjs`), because Xcode takes them from the archive when it exports. A separate job on a fresh runner then exports the archive with the App Store Connect API key: Xcode makes or fetches the distribution certificate and the profiles for the app and its extensions itself. A fresh runner has no development certificate, so Xcode makes one each run; the run first revokes the ones earlier runs made, named "Created via API", before Apple's certificate limit refuses a new one. Internal testers get each build once Apple has processed it. The run then waits for that processing and sets "What to Test" from `mobile/RELEASE_NOTES.md` (`mobile/ios/scripts/testflight-release.mjs internal`). A stable tag builds nothing for the iPhone: it submits its nightly's build for App Store review, set to release itself once Apple approves it (`testflight-release.mjs app-store`); an older version still open for editing is renamed to this one, since Apple keeps one open at a time. The iPhone app has no over-the-air updates: every change reaches it as a new build. The archive's dSYMs, with the Swift's and the Rust client's debug info, go to the `sikemux-mobile` Sentry project with `SENTRY_AUTH_TOKEN`, and only this build carries the Sentry address, so dev builds never report.

The app brings its own standard encryption (TLS and QUIC from rustls) and is not sold in France. For that answer, standard algorithms in addition to Apple's and no sale in France, App Store Connect records a build as using no non-exempt encryption, so the app says so itself with `ITSAppUsesNonExemptEncryption` set to false and builds skip the question. Selling in France needs a French encryption declaration first, and then the key changes to the code App Store Connect gives for it.

| Name                          | Kind   | Holds                                                     |
| ----------------------------- | ------ | --------------------------------------------------------- |
| `APP_STORE_CONNECT_KEY`       | secret | the "Sikemux CI" App Store Connect API key (Admin), `.p8` |
| `APP_STORE_CONNECT_KEY_ID`    | secret | its key id                                                |
| `APP_STORE_CONNECT_ISSUER_ID` | secret | the team's issuer id                                      |

Locally the key is at `~/.config/sikemux/release/app-store-connect-W6MR4F94W3.p8`.

Locally, `pnpm android:release` in `mobile/` signs with the same key: the keystore from `~/.config/sikemux/release/upload.keystore` and its password from the Keychain entry "Sikemux Android upload key".

### Over-the-air updates

Only the Android app updates over the air. A release build asks `updates.sikemux.com` for newer JavaScript and assets each time it starts, and again when it comes to the front and every five minutes while it stays there, at most once every five minutes, on its own channel: a nightly build on `nightly`, a stable one on `stable`. It restarts into a downloaded update the next time the person comes back after more than a minute away, and the account sheet offers Restart to update until then. The **Mobile release** run tells the build its channel through `SIKEMUX_MOBILE_CHANNEL`. Dev builds never update.

Every push to `main` that changes the phone's JavaScript or assets runs the **Mobile update** workflow; `mobile/app/test/updatePaths.test.ts` fails when the bundle reads a file its path list misses. One job checks the phone app and exports it with `expo export`; a second, which sees only the exported files, signs the update and sends it to citadel on `nightly`. The signature is checked against `mobile/app/certs/updates-certificate.pem` before anything is sent, and phones refuse an update without it.

An update carries a runtime version, a fingerprint of everything native in the app: the Expo config, native modules, and the Rust the phone links, found by following `Cargo.lock` from `sikemux-mobile` (see `mobile/app/scripts/rust-sources.js`). A desktop release or a desktop-only dependency does not change it. A phone only takes updates with its own build's runtime version, so when no phone release lists the update's runtime in its `runtime-android.txt`, the run skips publishing with a warning: the update would reach no phone, and the next phone release ships that code itself. A nightly phone release runs **Mobile update** on `main` once its APK is published, so code pushed while it was building, which no release matched yet, reaches the new build. The run's summary shows the update id and runtime version.

To promote a nightly update to stable, run **Mobile update** by hand with its id and the action `promote`. It serves the same signed update on `stable`.

A bad update is taken back with the action `withdraw`, then fixed forward. `withdraw` stops offering the update on every channel, so phones that have not taken it never do; the channel goes back to what it offered before, by publish or promotion time. Phones that already run it keep it until a fix pushed to `main` reaches them, except that one that crashes as it starts goes back to the code it ran before by itself.

Both actions need `server/deploy/receive-release` and the `lib.sh` beside it on citadel to be the current ones; they are installed by `setup-citadel.sh`, not by a server deploy.

Locally, `node scripts/publish-update.mjs android nightly --dry-run` in `mobile/app` builds and signs an update without sending it, with the key from `UPDATES_SIGNING_KEY`. The key lives at `~/.config/sikemux/release/updates-signing-key.pem`.

| Name                  | Kind   | Environment  | Holds                                |
| --------------------- | ------ | ------------ | ------------------------------------ |
| `UPDATES_SIGNING_KEY` | secret | `production` | the update signing key, PEM          |
| `DEPLOY_SSH_KEY`      | secret | `production` | the `sikemux-deploy` key for citadel |
| `DEPLOY_KNOWN_HOSTS`  | secret | `production` | citadel's host key                   |
| `DEPLOY_HOST`         | secret | `production` | citadel's address                    |
