#!/usr/bin/env bash
# Credential-free, offline checks for platform release configuration and tooling.
set -euo pipefail

cd "$(dirname "$0")/.."
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

bash -n scripts/build-mac.sh scripts/release.sh scripts/sign-helper.sh scripts/sign-mac.sh scripts/mac-entitlements.sh scripts/verify-release.sh scripts/publish-release.sh scripts/sign-host.sh scripts/icons.sh scripts/check-release.sh scripts/check-rust.sh scripts/sentry-release.sh scripts/keep-linux-symbols.sh
bash -n packaging/arch/PKGBUILD packaging/arch/prepare.sh packaging/arch/build-release.sh packaging/arch/push-aur.sh packaging/homebrew/push-cask.sh
node --check scripts/verify-updater-signature.mjs
node --check scripts/app-metadata.mjs
node --check scripts/release-credits.mjs
node --check scripts/build-core.mjs
node --check scripts/smoke-core.mjs
node --check scripts/build-voice-helper.mjs
node --check scripts/build-notch-helper.mjs
node scripts/generate-notch-icons.mjs --check
node --check scripts/build-sim-helper.mjs
python3 -c 'import plistlib, sys; plistlib.load(open(sys.argv[1], "rb"))' packaging/Info.plist

if scripts/release.sh 0.2.0 fixture --nightly >/dev/null 2>&1; then
  echo "release tooling accepted a stable semver for the nightly channel" >&2
  exit 1
fi
if scripts/release.sh 0.2.0-beta.1 fixture >/dev/null 2>&1; then
  echo "release tooling accepted a prerelease semver for the stable channel" >&2
  exit 1
fi

node - <<'NODE'
const fs = require("node:fs");
const pkg = JSON.parse(fs.readFileSync("package.json", "utf8"));
const app = JSON.parse(require("node:child_process").execFileSync("node", ["scripts/app-metadata.mjs"], { encoding: "utf8" }));
const macBuild = fs.readFileSync("scripts/build-mac.sh", "utf8");
const linuxBuild = fs.readFileSync("scripts/build-linux.sh", "utf8");
const installer = fs.readFileSync("scripts/install.sh", "utf8");
const signHost = fs.readFileSync("scripts/sign-host.sh", "utf8");
const coreBuild = fs.readFileSync("scripts/build-core.mjs", "utf8");
const publish = fs.readFileSync("scripts/publish-release.sh", "utf8");
const releaseWorkflow = fs.readFileSync(".github/workflows/release.yml", "utf8");
const pkgbuild = fs.readFileSync("packaging/arch/PKGBUILD", "utf8");
const ciWorkflow = fs.readFileSync(".github/workflows/ci.yml", "utf8");
const hakari = fs.readFileSync(".config/hakari.toml", "utf8");
const fail = (message) => { throw new Error(message); };

const backups = fs.readFileSync("server/deploy/backup-database", "utf8");
const promised = JSON.parse(fs.readFileSync("server/protocol/src/release.json", "utf8")).account.backupDays;
const prunedHours = Number(/^prune_older_than=(\d+)h$/m.exec(backups)?.[1]);
if (Math.ceil(prunedHours / 24) !== promised) fail(`backups are deleted after ${prunedHours}h, but the app promises ${promised} days`);

for (const gate of ["fmt", "hakari", "clippy", "no-plugins", "gpui-clippy", "gpui-test"]) {
  if (!ciWorkflow.includes(`run: scripts/check-rust.sh ${gate}\n`)) fail(`CI does not run the ${gate} gate pnpm check runs`);
}
for (const half of ["core-unit-tests", "other-tests"]) {
  if (!ciWorkflow.includes(`run: scripts/check-rust.sh ${half}\n`)) fail(`CI does not run the ${half} half of the Rust tests`);
}
if (!ciWorkflow.includes("run: pnpm check:helpers\n")) fail("CI does not build the helpers pnpm check builds");
for (const crate of ["sikemux-app", "sikemux-gpui", ...fs.readdirSync("app/crates")]) {
  if (!hakari.includes(`"${crate}"`)) fail(`.config/hakari.toml lets workspace-hack traverse ${crate}, which would bring GPUI's dependencies into the core's builds`);
}
if (pkg.version !== app.version) fail("package.json and the GPUI app's versions differ");
if (app.name !== "Sikemux" || app.identifier !== "com.nodelike.sikemux") fail("the app's name or identifier changed");
if (app["dev-name"] !== "Sikemux Dev" || app["dev-identifier"] !== "com.nodelike.sikemux.dev") fail("the dev app's name or identifier changed");
if (!macBuild.includes("cargo build --release --locked --bin sikemux")) fail("macOS build does not build the GPUI app");
if (!macBuild.includes('/usr/bin/hdiutil convert "$LAYOUT.rw.dmg" -format ULMO')) fail("macOS build does not pack the DMG");
if (!macBuild.includes("_aarch64.dmg")) fail("the DMG's name lost the _aarch64.dmg suffix sikemux.com finds its Download button by");
if (!macBuild.includes('"$ROOT/packaging/icons/build/Assets.car" "$APP_PATH/Contents/Resources/Assets.car"')) fail("Assets.car is not bundled");
if (!macBuild.includes('-c "Merge $ROOT/packaging/Info.plist"')) fail("pre-signing Info.plist merge is not configured");
if (app["minimum-macos"] !== "11.0") fail("unexpected minimum macOS version");
if (!app.description) fail("the app has no description for its Linux packages");
if (!macBuild.includes('--entitlements "$ROOT/packaging/Entitlements.plist"')) fail("microphone entitlement is not configured");
if (pkg.scripts?.["build:linux"] !== "scripts/build-linux.sh") fail("pnpm build:linux does not run scripts/build-linux.sh");
if (!linuxBuild.includes("node scripts/build-core.mjs")) fail("Linux build does not build the core");
if (!linuxBuild.includes("cargo build --release --locked --bin sikemux")) fail("Linux build does not build the GPUI app");
for (const name of ["Sikemux_${VERSION}_${DEB_ARCH}.deb", "Sikemux-${VERSION}-1.${ARCH}.rpm", "Sikemux_${VERSION}_${DEB_ARCH}.AppImage"]) {
  if (!linuxBuild.includes(name)) fail(`Linux build does not make ${name}`);
  if (!installer.includes(name)) fail(`the installer does not download ${name}`);
}
if (!linuxBuild.includes("sha256sum -c")) fail("Linux build runs packaging tools it has not checked");
if (!linuxBuild.includes('"$BUNDLE/tar/Sikemux_${VERSION}_${DEB_ARCH}.tar.gz"')) fail("Linux build does not make the tarball the Arch package is made from");
if (!pkgbuild.includes("/Sikemux_${_version}_amd64.tar.gz")) fail("the PKGBUILD does not download the tarball the Linux build makes");
if (!installer.includes("pacman -U --noconfirm") || !installer.includes("-1-x86_64.pkg.tar.zst")) fail("the installer does not install the Arch package with pacman");
if (!releaseWorkflow.includes("packaging/arch/build-release.sh")) fail("the Release workflow does not build and install the Arch package");
if (!publish.includes('"${SYMBOLS[@]}" "${TARBALLS[@]}" "$PACMAN_PACKAGE")')) fail("releases do not publish the Linux tarball and the Arch package");
if (/TAURI_SIGNING|signer sign/.test(linuxBuild)) fail("Linux builds must not require updater credentials");
if (!releaseWorkflow.includes("run: pnpm build:linux")) fail("the Release workflow does not build the Linux packages");
if (!publish.includes('"${DEBS[@]}" "${RPMS[@]}" "${APPIMAGES[@]}"')) fail("releases do not publish the Linux packages");
if (!releaseWorkflow.includes("scripts/package-host.sh")) fail("the Release workflow does not build the Linux host core");
if (!publish.includes('"${APPIMAGES[@]}" "${HOSTS[@]}" "${HOST_SIGS[@]}"')) fail("releases do not publish the Linux host core and its signatures");
if (!publish.includes('"${HOST_SIGS[@]}" "$INSTALLER"')) fail("releases do not publish the installer made for their files");
if (!releaseWorkflow.includes("scripts/sign-host.sh")) fail("the Release workflow does not sign the Linux host core");
if (!signHost.includes('"${HOSTS[@]}" "${APPIMAGES[@]}"')) fail("releases do not sign the AppImages with the updater key");
if (!publish.includes('"$INSTALLER" "${APPIMAGE_SIGS[@]}"')) fail("releases do not publish the AppImage's update signature");
if (!publish.includes('"${APPIMAGE_SIGS[@]}" "${SYMBOLS[@]}"')) fail("releases do not publish the Mac dSYMs that name a crash's frames");
if (!releaseWorkflow.includes('run: scripts/sentry-release.sh "$VERSION" target/symbols')) fail("the Release workflow does not send Sentry the symbols that name a crash's frames");
if ((releaseWorkflow.match(/scripts\/keep-linux-symbols\.sh/g) ?? []).length !== 3) fail("the Release workflow does not keep the symbols of each Linux binary it builds");
if (!publish.includes('manifest["platforms"][f"linux-{arch}"]')) fail("the update feed does not offer the AppImage");
if (!publish.includes('manifest["platforms"][f"host-linux-{arch}"]')) fail("the update feed does not offer the host cores");
if (!releaseWorkflow.includes("scripts/smoke-host-install.sh")) fail("the Release workflow does not install the host core with its own installer");
const helpers = fs.readFileSync("scripts/release-shape.sh", "utf8").match(/^PUBLISHED_HELPERS=\((.*)\)$/m)?.[1].split(" ") ?? [];
if (helpers.join(" ") !== "sikemux-voice sikemux-sim") fail("a release no longer publishes the voice and simulator helpers the app downloads");
for (const helper of helpers) {
  if (!fs.existsSync(`scripts/build-${helper.replace("sikemux-", "")}-helper.mjs`)) fail(`no build script for ${helper}`);
}
if (!macBuild.includes('build-${helper#sikemux-}-helper.mjs')) fail("macOS build does not build the published helpers");
if (!macBuild.includes("build-notch-helper.mjs")) fail("macOS build does not build the notch helper");
if (!macBuild.includes('"$ROOT/packaging/binaries/notch/Sikemux Notch.app" "$APP_PATH/Contents/Helpers/Sikemux Notch.app"')) fail("the macOS bundle must carry the notch helper app");
if (!publish.includes('"$SIG" "${HELPERS[@]}" "${DEBS[@]}"')) fail("releases do not publish the helpers the app downloads");
if (!linuxBuild.includes('"$PI_TOOLS" "$PACKAGE/usr/lib/sikemux/sikemux_pi_tools.ts"')) fail("Linux packages do not carry the Pi browser extension");
if (!macBuild.includes("build-core.mjs")) fail("macOS build does not build the core");
if (!macBuild.includes('"$ROOT/packaging/binaries/core/Helpers/Sikemux Core.app" "$APP_PATH/Contents/Helpers/Sikemux Core.app"')) fail("the macOS bundle must carry the core app");
if (app["core-link"] !== "../Helpers/Sikemux Core.app/Contents/MacOS/sikemux-host" || !macBuild.includes('ln -s "$CORE_LINK_TARGET" "$APP_PATH/Contents/MacOS/sikemux-host"')) fail("the macOS bundle must link Contents/MacOS/sikemux-host to the core");
for (const profile of ["sikemux-developer-id", "sikemux-core-developer-id"]) {
  if (!fs.existsSync(`packaging/macos/${profile}.provisionprofile`)) fail(`packaging/macos/${profile}.provisionprofile is missing`);
}
if (!coreBuild.includes("smokeCore")) fail("the core build does not run its smoke test");
for (const [name, text] of [["scripts/build-mac.sh", macBuild], ["scripts/build-core.mjs", coreBuild], [".github/workflows/release.yml", releaseWorkflow]]) {
    if (/--all-features|--features[ =][^\n]*\b(drive|fake-plugins)\b/.test(text)) fail(`${name} turns on a test kit's feature`);
}
if (!macBuild.includes("this test kit has no answer for that") || !macBuild.includes("window driver could not listen")) {
    fail("macOS build no longer refuses a binary built with a test kit's feature");
}
const updates = fs.readFileSync("crates/sikemux-corekit/src/release_feed.rs", "utf8");
if (!updates.includes('"https://github.com/nodelike/sikemux/releases/latest/download/latest.json"')) {
  fail("the stable update feed is not the expected HTTPS latest.json URL");
}
if (!updates.includes('include_str!("../../../app/Cargo.toml")')) fail("the core does not take the updater key from the GPUI app's Cargo.toml");
const publicKeyText = Buffer.from(app["updater-key"], "base64").toString("utf8").trim();
const publicLines = publicKeyText.split("\n");
if (publicLines.length !== 2 || !publicLines[0].startsWith("untrusted comment:")) fail("invalid updater public-key envelope");
const packet = Buffer.from(publicLines[1], "base64");
if (packet.length !== 42 || packet.subarray(0, 2).toString("ascii") !== "Ed") fail("invalid updater Ed25519 public key");
NODE

# Shipped clients resolve this exact URL, so the endpoint is a fixed contract.
grep -Fq 'releases/download/nightly/latest.json' crates/sikemux-corekit/src/release_feed.rs || {
  echo "nightly updater endpoint is missing" >&2
  exit 1
}
# shellcheck disable=SC2016
grep -Fq 'gh release upload nightly "$MANIFEST" --clobber' scripts/publish-release.sh || {
  echo "nightly feed no longer repoints its latest.json" >&2
  exit 1
}
# shellcheck disable=SC2016
grep -Fq 'RELEASE_CMD=(gh release create "v$VERSION"' scripts/publish-release.sh || {
  echo "nightly channel does not publish an immutable versioned release" >&2
  exit 1
}
# shellcheck disable=SC2016
if grep -E 'gh release (create|upload|edit) nightly' scripts/publish-release.sh | grep -qE '\$(DMG|TAR|SIG)'; then
  echo "nightly channel still attaches builds to the moving pointer release" >&2
  exit 1
fi
# shellcheck disable=SC2016
grep -Fq 'MANIFEST="$BUNDLE/latest.json"' scripts/verify-release.sh || {
  echo "nightly manifest is not written beside its artifacts" >&2
  exit 1
}
grep -Fq 'pathlib.Path("latest.json").write_text' scripts/verify-release.sh && {
  echo "release tooling still hardcodes the repo-root manifest path" >&2
  exit 1
}
grep -Fq 'stable releases must be cut from a release/<line> branch' scripts/release.sh || {
  echo "stable releases are not pinned to a release branch" >&2
  exit 1
}
grep -Fq 'nightly releases must be cut from main' scripts/release.sh || {
  echo "nightly releases are not pinned to main" >&2
  exit 1
}
# shellcheck disable=SC2016
grep -Fq '[[ "${GITHUB_ACTIONS:-}" == "true" ]] || fail "releases publish from the Release workflow' scripts/release.sh || {
  echo "release tooling can publish from outside the Release workflow" >&2
  exit 1
}
# shellcheck disable=SC2016
grep -Fq './scripts/release.sh "$VERSION" "$(cat RELEASE_NOTES.md)" "${flags[@]}"' .github/workflows/release.yml || {
  echo "the Release workflow no longer builds through scripts/release.sh" >&2
  exit 1
}
# shellcheck disable=SC2016
grep -Fq './scripts/publish-release.sh "$VERSION" "$(cat RELEASE_NOTES.md)"' .github/workflows/release.yml || {
  echo "the Release workflow no longer publishes through scripts/publish-release.sh" >&2
  exit 1
}
grep -Fq 'group: release' .github/workflows/release.yml || {
  echo "the Release workflow allows concurrent releases" >&2
  exit 1
}

channel="$(sed -n 's/^channel = "\(.*\)"$/\1/p' rust-toolchain.toml)"
grep -Fq "container: rust:${channel%.*}-bookworm" .github/workflows/release.yml || {
  echo "the Release workflow's container is not on rust-toolchain.toml's Rust" >&2
  exit 1
}

CARGO_METADATA="$(cargo metadata --offline --locked --no-deps --format-version 1)"
PACKAGE_VERSION="$(node -p "require('./package.json').version")"
CARGO_VERSION="$(node -e '
  const metadata = JSON.parse(require("fs").readFileSync(0, "utf8"));
  const host = metadata.packages.find((pkg) => pkg.name === "sikemux-host");
  if (!host) process.exit(1);
  process.stdout.write(host.version);
' <<<"$CARGO_METADATA")"
[[ "$PACKAGE_VERSION" == "$CARGO_VERSION" ]] || {
  echo "package.json and Cargo package versions differ" >&2
  exit 1
}

# Exercise the verifier against a fresh ephemeral key and prove that changing
# the signed bytes, or signing with another key, is rejected. The fixture signs
# as minisign does for the updater: Ed25519 over the file's BLAKE2b-512, then
# over that signature and its trusted comment. No project key or network is used.
printf 'offline updater verifier fixture\n' >"$TMP/artifact.tar.gz"
node - "$TMP" <<'NODE'
const crypto = require("node:crypto");
const fs = require("node:fs");
const dir = process.argv[2];
const artifact = fs.readFileSync(`${dir}/artifact.tar.gz`);
const minisign = (name) => {
  const { publicKey, privateKey } = crypto.generateKeyPairSync("ed25519");
  const id = crypto.randomBytes(8);
  const raw = Buffer.from(publicKey.export({ format: "jwk" }).x, "base64url");
  const signature = crypto.sign(null, crypto.createHash("blake2b512").update(artifact).digest(), privateKey);
  const trusted = "timestamp:0\tfile:artifact.tar.gz";
  const global = crypto.sign(null, Buffer.concat([signature, Buffer.from(trusted)]), privateKey);
  const pub = `untrusted comment: minisign public key\n${Buffer.concat([Buffer.from("Ed"), id, raw]).toString("base64")}\n`;
  const sig = `untrusted comment: signature\n${Buffer.concat([Buffer.from("ED"), id, signature]).toString("base64")}\ntrusted comment: ${trusted}\n${global.toString("base64")}\n`;
  fs.writeFileSync(`${dir}/${name}.pub`, Buffer.from(pub).toString("base64"));
  fs.writeFileSync(`${dir}/${name}.sig`, Buffer.from(sig).toString("base64"));
};
minisign("test");
minisign("other");
NODE
node scripts/verify-updater-signature.mjs "$TMP/artifact.tar.gz" "$TMP/test.sig" "$(cat "$TMP/test.pub")" >/dev/null
if node scripts/verify-updater-signature.mjs "$TMP/artifact.tar.gz" "$TMP/other.sig" "$(cat "$TMP/test.pub")" >"$TMP/other.out" 2>&1; then
  echo "updater verifier accepted an artifact signed with another key" >&2
  exit 1
fi
printf 'tampered\n' >>"$TMP/artifact.tar.gz"
if node scripts/verify-updater-signature.mjs "$TMP/artifact.tar.gz" "$TMP/test.sig" "$(cat "$TMP/test.pub")" >"$TMP/tamper.out" 2>&1; then
  echo "updater verifier accepted a modified artifact" >&2
  exit 1
fi

echo "✓ Release tooling static checks passed"
