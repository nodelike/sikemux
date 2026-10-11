# Releasing

Releases publish from the **Release** GitHub Actions workflow, never from a laptop. Commit the version bump and a `RELEASE_NOTES.md` headed `# Sikemux v<version>`, then push the matching tag. Tag a commit already on `main` for a nightly, or on its `release/<major.minor>` branch for stable. Only the owner can push a `v*` tag, so only the owner can start a release:

```bash
make preflight
git tag v0.4.1 && git push origin v0.4.1
```

`make preflight` runs the two checks the pre-push hook does not: it launches the real app the way CI's desktop E2E job does, and builds the DMG against its size limit. Either failing would otherwise surface only in the Release run.

A run is titled with its tag, so the approval names what it will publish, and it stops if the tag disagrees with `package.json`. The workflow reads the version from `package.json` and builds and verifies with `scripts/release.sh` while the full CI suite runs. Once both pass, `scripts/publish-release.sh` publishes the build. A prerelease version goes to the nightly channel and any other version to stable. Only one release runs at a time, and each run keeps its built artifacts.

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

The `release` environment holds the secrets and has no reviewer. A stable release first waits on its "Approve the stable release" job, which runs on the `approval` environment: that one holds nothing and has the owner as its required reviewer. A nightly skips the approval and publishes as soon as its checks pass. The approval can be given while the checks are still running.

Run `scripts/release.sh` locally without `--publish` to preview a release: it builds, signs, and verifies everything without touching GitHub.

## Community releases without an Apple Developer membership

The updater and Apple Gatekeeper trust different signatures. By default, `scripts/release.sh` makes a community release. It signs the updater archive with the Tauri updater key and applies an ad hoc code signature to the app and DMG.

Existing community installations can receive in-app updates. Fresh downloads are not notarized by Apple, so macOS may ask you to remove quarantine again. Keep the updater private key secure. Clients reject archives that do not match the public key bundled with the app.

Both channels create a versioned GitHub release holding the build. A stable cut also attaches `latest.json`, which the default channel follows. A nightly cut requires a prerelease semantic version, publishes its release as a prerelease, and repoints the moving `nightly` release that the opt-in Nightly channel follows.

Stable is cut from a `release/<major.minor>` branch and nightly from `main`. A nightly targets whichever version comes next, whether that is a patch, a minor or a major, and a stable release of that version overtakes its nightlies for nightly users too.

## Version numbers

A nightly is a prerelease of the next stable release. After a stable `0.x.y`, `main`'s nightlies are `0.x.(y+1)-nightly.N`, or `0.(x+1).0-nightly.N` when the next release is a bigger one. Stable releases come every few days, each a promoted nightly.

## Promoting a nightly to stable

A nightly that has been in use becomes stable without anything newer from `main`. On `release/<major.minor>`:

1. A merge commit whose content is exactly that nightly's tagged commit.
2. A commit that sets the stable version in `package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml` and the `sikemux` entry in `src-tauri/Cargo.lock`, and writes `RELEASE_NOTES.md` for everything since the last stable release.
3. The `v<version>` tag on that commit.

None of this lands on `main`. Commits made to `main` meanwhile stay out of the release.

## Hotfixes

A hotfix is the next release, shipped early. Fix the bug on `main`, ship a nightly, and promote that nightly to stable. It takes the version the nightlies were already building toward, so nothing collides, and it carries whatever else is on `main`.

Cherry-pick onto `release/<major.minor>` only when `main` holds something too risky to ship, such as a half-finished refactor during a security fix. The cherry-picked hotfix then takes the version the nightlies are building toward, and the updater takes the newest version across both channels, so first bump `main` to the patch after it and publish that nightly. Then cherry-pick only the fix onto the release branch, bump to the claimed patch, write short notes and tag it. Never merge a release branch into `main`, or `main` into a release branch.

## Committing to a release branch from the shared checkout

Several agents work in one checkout at once, so never check out `release/*`, stash, or use a worktree. Build the commits from objects with a temporary index and push them by id. Use `${name}:path`, not `$name:path`: zsh reads `:s`, `:h` and the like after a bare variable as modifiers.

Promote a nightly:

```bash
release=release/0.4 nightly=v0.4.3-nightly.6 version=0.4.3
git fetch origin
base=$(git rev-parse "origin/${release}"); src=$(git rev-parse "${nightly}^{commit}")
merge=$(git commit-tree "${src}^{tree}" -p "${base}" -p "${src}" -m "merge: bring ${nightly} onto the ${release#release/} line for v${version}")
export GIT_INDEX_FILE=$(mktemp -u); git read-tree "${merge}"
from=$(git show "${merge}:package.json" | sed -n 's/.*"version": "\(.*\)".*/\1/p')
put() { git update-index --cacheinfo "100644,$(git hash-object -w --stdin),$1"; }
git show "${merge}:package.json" | sed "s/\"version\": \"${from}\"/\"version\": \"${version}\"/" | put package.json
git show "${merge}:src-tauri/tauri.conf.json" | sed "s/\"version\": \"${from}\"/\"version\": \"${version}\"/" | put src-tauri/tauri.conf.json
git show "${merge}:src-tauri/Cargo.toml" | sed "3s/\"${from}\"/\"${version}\"/" | put src-tauri/Cargo.toml
git show "${merge}:src-tauri/Cargo.lock" | awk -v from="${from}" -v to="${version}" 'prev=="name = \"sikemux\"" && $0=="version = \""from"\"" {print "version = \""to"\""; prev=$0; next} {print; prev=$0}' | put src-tauri/Cargo.lock
put RELEASE_NOTES.md < /path/to/notes.md
bump=$(git commit-tree "$(git write-tree)" -p "${merge}" -m "chore(release): prepare v${version}")
rm -f "${GIT_INDEX_FILE}"; unset GIT_INDEX_FILE
git diff --stat "${merge}" "${bump}"   # only the four version fields and the notes
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

If you have an Apple Developer membership, set `RELEASE_NOTARIZED=1` with the Developer ID and notarization environment variables. The release script then requires a successful Gatekeeper assessment and stapled notarization tickets before it publishes anything.

## Releasing the phone app

The phone app releases on its own schedule from the **Mobile release** workflow, started by a `mobile-v*` tag. Only the owner can create, move or delete one. Like a Mac release, a stable one waits for the owner's approval on the `release` environment, and a nightly publishes once its checks pass.

```bash
git tag mobile-v0.1.0-nightly.1 && git push origin mobile-v0.1.0-nightly.1
```

`mobile/app/app.json` holds the plain version, such as `0.1.0`, because iOS refuses anything else. A tag is either that version, which is stable, or that version with `-nightly.N`. The run stops if the tag is not a release of the version in `app.json`.

Android needs a version code that grows with every upload. `app.config.js` derives it from the tag: `0.5.0-nightly.3` is `50003`, and `0.5.0` itself is `50099`, so a stable build always follows its own nightlies. Minor and patch numbers stay below 100, and nightlies below 99. The iOS build number is the same code, and the app reads its release back from it to compare with the oldest version `GET /v1/network` allows.

The run checks the phone app, builds the Rust client with the small `mobile` profile, and builds the APK and the Play app bundle. It refuses either unless it is signed with the Play upload key. It attaches the APK to a GitHub release of the tag, marked a prerelease for a nightly, and never as the latest release: sikemux.com takes its Mac download from that one. It also attaches `runtime-android.txt`, the runtime version the build was made for (see below). Last, it puts the app bundle on Google Play: a nightly on the closed testing track, where testers get it at once, and a stable release on production, where it starts with 10% of phones. Once it holds up, widen it with `node scripts/play-release.mjs rollout 0.5` in `mobile/app`, then `rollout 1` for every phone, with the service account's key in `PLAY_SERVICE_ACCOUNT`. Google Play keeps production closed until the closed test has had 12 testers for 14 days, so a stable tag fails at that step until then.

Every phone release needs its notes in `mobile/RELEASE_NOTES.md`, committed before the tag: a `# <version>` heading, then what changed in plain text, at most 500 characters. Google Play shows them as "What's new" and the GitHub release opens with them. The run checks the file before it builds and stops if the heading is for another version, so `node scripts/play-release.mjs notes <version>` in `mobile/app` shows what Google Play will get.

| Name                      | Kind   | Holds                                                                                         |
| ------------------------- | ------ | --------------------------------------------------------------------------------------------- |
| `ANDROID_UPLOAD_KEYSTORE` | secret | the upload keystore, base64                                                                   |
| `ANDROID_UPLOAD_PASSWORD` | secret | its password, which is also the key's own password                                            |
| `PLAY_SERVICE_ACCOUNT`    | secret | the `sikemux-play-release` service account's JSON key, allowed to release in the Play Console |

The same run builds the iPhone app on a macOS runner and sends it to TestFlight, as build number the version code above. Xcode signs it with the App Store Connect API key: it makes or fetches the distribution certificate and the profiles for the app and its notification extension itself. Internal testers get each build once Apple has processed it. The run then waits for that processing, sets "What to Test" from `mobile/RELEASE_NOTES.md`, adds the build to the external `Testers` group and submits it for beta review (`mobile/app/scripts/testflight-release.mjs`), so external testers get every nightly and stable build once Apple approves it. A stable build is then submitted for App Store review by hand.

The app brings its own standard encryption (TLS and QUIC from rustls) and is not sold in France. For that answer, standard algorithms in addition to Apple's and no sale in France, App Store Connect records a build as using no non-exempt encryption, so the app says so itself with `ITSAppUsesNonExemptEncryption` set to false and builds skip the question. Selling in France needs a French encryption declaration first, and then the key changes to the code App Store Connect gives for it.

| Name                          | Kind   | Holds                                                     |
| ----------------------------- | ------ | --------------------------------------------------------- |
| `APP_STORE_CONNECT_KEY`       | secret | the "Sikemux CI" App Store Connect API key (Admin), `.p8` |
| `APP_STORE_CONNECT_KEY_ID`    | secret | its key id                                                |
| `APP_STORE_CONNECT_ISSUER_ID` | secret | the team's issuer id                                      |

Locally the key is at `~/.config/sikemux/release/app-store-connect-W6MR4F94W3.p8`.

Locally, `pnpm android:release` in `mobile/` signs with the same key: the keystore from `~/.config/sikemux/release/upload.keystore` and its password from the Keychain entry "Sikemux Android upload key".

### Over-the-air updates

A release build asks `updates.sikemux.com` for newer JavaScript and assets each time it starts, and again whenever it comes to the front, at most every half hour, on its own channel: a nightly build on `nightly`, a stable one on `stable`. It restarts into a downloaded update the next time the person comes back after more than a minute away, and the account sheet offers Restart to update until then. The **Mobile release** run tells the build its channel through `SIKEMUX_MOBILE_CHANNEL`. Dev builds never update.

Every push to `main` that changes the phone's JavaScript or assets runs the **Mobile update** workflow; `mobile/app/test/updatePaths.test.ts` fails when the bundle reads a file its path list misses. One job checks the phone app and exports it with `expo export`; a second, which sees only the exported files, signs the update and sends it to citadel on `nightly`. The signature is checked against `mobile/app/certs/updates-certificate.pem` before anything is sent, and phones refuse an update without it.

An update carries a runtime version, a fingerprint of everything native in the app: the Expo config, native modules, and the Rust the phone links, found by following `src-tauri/Cargo.lock` from `sikemux-mobile` (see `mobile/app/scripts/rust-sources.js`). A desktop release or a desktop-only dependency does not change it. A phone only takes updates with its own build's runtime version, so when no phone release lists the update's runtime in its `runtime-android.txt`, the run skips publishing with a warning: the update would reach no phone, and the next phone release ships that code itself. A nightly phone release runs **Mobile update** on `main` once its APK is published, so code pushed while it was building, which no release matched yet, reaches the new build. The run's summary shows the update id and runtime version.

To promote a nightly update to stable, run **Mobile update** by hand with its id and the action `promote`. It waits for the owner's approval on the `release` environment and then serves the same signed update on `stable`.

A bad update is taken back the same way, with one of two actions. Each channel serves what was put on it last, by publish or promotion time.

- `withdraw` stops offering the update on every channel, so phones that have not taken it never do; the channel goes back to what it offered before. Phones that already run it keep it.
- `roll-back` also withdraws it, and sends phones that run it a signed order to go back to the code in their build. Only updates prepared after the roll-back reach those phones again, so fix forward with a new push to `main`.

Both need `server/deploy/receive-release` on citadel to be the current one; it is installed by `setup-citadel.sh`, not by a server deploy.

Locally, `node scripts/publish-update.mjs android nightly --dry-run` in `mobile/app` builds and signs an update without sending it, with the key from `UPDATES_SIGNING_KEY`. The key lives at `~/.config/sikemux/release/updates-signing-key.pem`.

| Name                  | Kind   | Environment  | Holds                                |
| --------------------- | ------ | ------------ | ------------------------------------ |
| `UPDATES_SIGNING_KEY` | secret | `production` | the update signing key, PEM          |
| `DEPLOY_SSH_KEY`      | secret | `production` | the `sikemux-deploy` key for citadel |
| `DEPLOY_KNOWN_HOSTS`  | secret | `production` | citadel's host key                   |
| `DEPLOY_HOST`         | secret | `production` | citadel's address                    |
