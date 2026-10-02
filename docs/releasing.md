# Releasing

Releases publish from the **Release** GitHub Actions workflow, never from a laptop. Commit the version bump and a `RELEASE_NOTES.md` headed `# Sikemux v<version>`, then push the matching tag. Tag a commit already on `main` for a nightly, or on its `release/<major.minor>` branch for stable. Only the owner can push a `v*` tag, so only the owner can start a release:

```bash
make preflight
git tag v0.4.1 && git push origin v0.4.1
```

`make preflight` runs the two checks the pre-push hook does not: it launches the real app the way CI's desktop E2E job does, and builds the DMG against its size limit. Either failing would otherwise surface only in the Release run.

A run is titled with its tag, so the approval names what it will publish, and it stops if the tag disagrees with `package.json`. The workflow reads the version from `package.json`, runs the full CI suite, then builds, verifies, and publishes with `scripts/release.sh`. A prerelease version goes to the nightly channel and any other version to stable. Only one release runs at a time, and each run keeps its built artifacts.

If a release fails before it publishes, fix it and move the tag onto the fix. Only the owner can move a release tag, and moving it starts a fresh run:

```bash
git tag -f v0.4.1 && git push -f origin v0.4.1
```

The workflow takes its signing material from the `release` environment:

| Name                                                                                                                                       | Kind                    | Needed for         |
| ------------------------------------------------------------------------------------------------------------------------------------------ | ----------------------- | ------------------ |
| `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`                                                                          | secret                  | every release      |
| `RELEASE_NOTARIZED`                                                                                                                        | variable, `1` to enable | notarized releases |
| `APPLE_CERTIFICATE` (base64 `.p12`), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | secrets                 | notarized releases |

Limit the environment's deployment refs to `main`, `release/*`, and `v*` tags, and add yourself as a required reviewer so nothing publishes unapproved.

Run `scripts/release.sh` locally without `--publish` to preview a release: it builds, signs, and verifies everything without touching GitHub.

## Community releases without an Apple Developer membership

The updater and Apple Gatekeeper trust different signatures. By default, `scripts/release.sh` makes a community release. It signs the updater archive with the Tauri updater key and applies an ad hoc code signature to the app and DMG.

Existing community installations can receive in-app updates. Fresh downloads are not notarized by Apple, so macOS may ask you to remove quarantine again. Keep the updater private key secure. Clients reject archives that do not match the public key bundled with the app.

Both channels create a versioned GitHub release holding the build. A stable cut also attaches `latest.json`, which the default channel follows. A nightly cut requires a prerelease semantic version, publishes its release as a prerelease, and repoints the moving `nightly` release that the opt-in Nightly channel follows.

Stable is cut from a `release/<major.minor>` branch and nightly from `main`. A nightly targets whichever version comes next, whether that is a patch, a minor or a major, and a stable release of that version overtakes its nightlies for nightly users too.

A hotfix cut from a release branch claims a version as well. When it claims the one the nightlies are building toward, the Nightly channel moves onto the hotfix, because the updater takes the newest version across both feeds, and loses whatever `main` had that the hotfix did not until a later nightly passes it. Before cutting such a hotfix, publish a nightly at the version after it, so the hotfix lands below the nightlies instead of over them.

```bash
./scripts/release.sh 0.3.5 "Release notes"
./scripts/release.sh 0.4.0-nightly.1 "Nightly notes" --nightly
```

If you have an Apple Developer membership, set `RELEASE_NOTARIZED=1` with the Developer ID and notarization environment variables. The release script then requires a successful Gatekeeper assessment and stapled notarization tickets before it publishes anything.
