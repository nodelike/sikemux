#!/usr/bin/env bash
# Credential-free, offline checks for platform release configuration and tooling.
set -euo pipefail

cd "$(dirname "$0")/.."
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

bash -n scripts/build-mac.sh scripts/release.sh scripts/sign-helper.sh scripts/sign-mac.sh scripts/mac-entitlements.sh scripts/verify-release.sh scripts/publish-release.sh scripts/sign-host.sh scripts/icons.sh scripts/check-release.sh
node --check scripts/verify-updater-signature.mjs
node --check scripts/app-metadata.mjs
node --check scripts/release-credits.mjs
node --check scripts/build-cli-sidecar.mjs
node --check scripts/smoke-browser-sidecar.mjs
node --check scripts/build-voice-helper.mjs
node --check scripts/build-notch-helper.mjs
node scripts/generate-notch-icons.mjs --check
node --check scripts/build-sim-helper.mjs
/usr/bin/plutil -lint src-tauri/Info.plist >/dev/null

if RELEASE_CHANNEL=preview scripts/release.sh 0.2.0-beta.1 fixture >/dev/null 2>&1; then
  echo "release tooling accepted an unsupported channel" >&2
  exit 1
fi
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
const sidecarBuild = fs.readFileSync("scripts/build-cli-sidecar.mjs", "utf8");
const publish = fs.readFileSync("scripts/publish-release.sh", "utf8");
const releaseWorkflow = fs.readFileSync(".github/workflows/release.yml", "utf8");
const fail = (message) => { throw new Error(message); };

if (pkg.version !== app.version) fail("package.json and the GPUI app's versions differ");
if (app.name !== "Sikemux" || app.identifier !== "com.nodelike.sikemux") fail("the app's name or identifier changed");
if (app["dev-name"] !== "Sikemux Dev" || app["dev-identifier"] !== "com.nodelike.sikemux.dev") fail("the dev app's name or identifier changed");
if (!macBuild.includes("cargo build --release --locked --bin sikemux")) fail("macOS build does not build the GPUI app");
if (!macBuild.includes('/usr/bin/hdiutil convert "$LAYOUT.rw.dmg" -format ULMO')) fail("macOS build does not pack the DMG");
if (!macBuild.includes('"$ROOT/src-tauri/icons/build/Assets.car" "$APP_PATH/Contents/Resources/Assets.car"')) fail("Assets.car is not bundled");
if (!macBuild.includes('-c "Merge $ROOT/src-tauri/Info.plist"')) fail("pre-signing Info.plist merge is not configured");
if (app["minimum-macos"] !== "11.0") fail("unexpected minimum macOS version");
if (!app.description) fail("the app has no description for its Linux packages");
if (!macBuild.includes('--entitlements "$ROOT/src-tauri/Entitlements.plist"')) fail("microphone entitlement is not configured");
if (pkg.scripts?.["build:linux"] !== "scripts/build-linux.sh") fail("pnpm build:linux does not run scripts/build-linux.sh");
if (!linuxBuild.includes("node scripts/build-cli-sidecar.mjs")) fail("Linux build does not build the core");
if (!linuxBuild.includes("cargo build --release --locked --bin sikemux")) fail("Linux build does not build the GPUI app");
for (const name of ["Sikemux_${VERSION}_amd64.deb", "Sikemux-${VERSION}-1.x86_64.rpm", "Sikemux_${VERSION}_amd64.AppImage"]) {
  if (!linuxBuild.includes(name)) fail(`Linux build does not make ${name}`);
  if (!installer.includes(name)) fail(`the installer does not download ${name}`);
}
if (!linuxBuild.includes("sha256sum -c")) fail("Linux build runs packaging tools it has not checked");
if (/TAURI_SIGNING|signer sign/.test(linuxBuild)) fail("Linux builds must not require updater credentials");
if (!releaseWorkflow.includes("run: pnpm build:linux")) fail("the Release workflow does not build the Linux packages");
if (!publish.includes('"$DEB" "$RPM" "$APPIMAGE"')) fail("releases do not publish the Linux packages");
if (!releaseWorkflow.includes("scripts/package-host.sh")) fail("the Release workflow does not build the Linux host core");
if (!publish.includes('"$APPIMAGE" "${HOSTS[@]}" "${HOST_SIGS[@]}"')) fail("releases do not publish the Linux host core and its signatures");
if (!publish.includes('"${HOST_SIGS[@]}" "$INSTALLER"')) fail("releases do not publish the installer made for their files");
if (!releaseWorkflow.includes("scripts/sign-host.sh")) fail("the Release workflow does not sign the Linux host core");
if (!signHost.includes('"${HOSTS[@]}" "$APPIMAGE"')) fail("releases do not sign the AppImage with the updater key");
if (!publish.includes('"$INSTALLER" "$APPIMAGE_SIG"')) fail("releases do not publish the AppImage's update signature");
if (!publish.includes('manifest["platforms"]["linux-x86_64"]')) fail("the update feed does not offer the AppImage");
if (!releaseWorkflow.includes("scripts/smoke-host-install.sh")) fail("the Release workflow does not install the host core with its own installer");
if (!macBuild.includes("build-voice-helper.mjs")) fail("macOS build does not build the voice helper");
if (!macBuild.includes("build-notch-helper.mjs")) fail("macOS build does not build the notch helper");
if (!macBuild.includes('"$ROOT/src-tauri/binaries/notch/Sikemux Notch.app" "$APP_PATH/Contents/Helpers/Sikemux Notch.app"')) fail("the macOS bundle must carry the notch helper app");
if (!publish.includes('"$SIG" "$VOICE"')) fail("releases do not publish the voice helper the app downloads");
if (!macBuild.includes("build-sim-helper.mjs")) fail("macOS build does not build the simulator helper");
if (!publish.includes('"$VOICE" "$SIM"')) fail("releases do not publish the simulator helper the app downloads");
if (!linuxBuild.includes('"$PI_TOOLS" "$PACKAGE/usr/lib/sikemux/sikemux_pi_tools.ts"')) fail("Linux packages do not carry the Pi browser extension");
if (!macBuild.includes("build-cli-sidecar.mjs")) fail("macOS build does not build the CLI sidecar");
if (!macBuild.includes('"$ROOT/src-tauri/binaries/core/Helpers/Sikemux Core.app" "$APP_PATH/Contents/Helpers/Sikemux Core.app"')) fail("the macOS bundle must carry the core app");
if (!macBuild.includes('ln -s "../Helpers/Sikemux Core.app/Contents/MacOS/sikemux-host" "$APP_PATH/Contents/MacOS/sikemux-host"')) fail("the macOS bundle must link Contents/MacOS/sikemux-host to the core");
for (const profile of ["sikemux-developer-id", "sikemux-core-developer-id"]) {
  if (!fs.existsSync(`src-tauri/macos/${profile}.provisionprofile`)) fail(`src-tauri/macos/${profile}.provisionprofile is missing`);
}
if (!sidecarBuild.includes("smokeBrowserSidecar")) fail("sidecar build does not run the browser smoke test");
for (const [name, text] of [["scripts/build-mac.sh", macBuild], ["scripts/build-cli-sidecar.mjs", sidecarBuild], [".github/workflows/release.yml", releaseWorkflow]]) {
    if (/--all-features|--features[ =][^\n]*\b(drive|fake-plugins)\b/.test(text)) fail(`${name} turns on a test kit's feature`);
}
if (!macBuild.includes("this test kit has no answer for that") || !macBuild.includes("window driver could not listen")) {
    fail("macOS build no longer refuses a binary built with a test kit's feature");
}
const updates = fs.readFileSync("src-tauri/crates/sikemux-core/src/server/updates.rs", "utf8");
if (!updates.includes('"https://github.com/nodelike/sikemux/releases/latest/download/latest.json"')) {
  fail("the stable update feed is not the expected HTTPS latest.json URL");
}
if (!updates.includes('include_str!("../../../sikemux-gpui/Cargo.toml")')) fail("the core does not take the updater key from the GPUI app's Cargo.toml");
const publicKeyText = Buffer.from(app["updater-key"], "base64").toString("utf8").trim();
const publicLines = publicKeyText.split("\n");
if (publicLines.length !== 2 || !publicLines[0].startsWith("untrusted comment:")) fail("invalid updater public-key envelope");
const packet = Buffer.from(publicLines[1], "base64");
if (packet.length !== 42 || packet.subarray(0, 2).toString("ascii") !== "Ed") fail("invalid updater Ed25519 public key");
NODE

# Shipped clients resolve this exact URL, so the endpoint is a fixed contract.
grep -Fq 'releases/download/nightly/latest.json' src-tauri/crates/sikemux-core/src/server/updates.rs || {
  echo "nightly updater endpoint is missing" >&2
  exit 1
}
# shellcheck disable=SC2016
grep -Fq 'gh release upload nightly "$MANIFEST" --clobber' scripts/publish-release.sh || {
  echo "nightly feed no longer repoints its latest.json" >&2
  exit 1
}
# shellcheck disable=SC2016
grep -Fq 'NIGHTLY_GH_CMD=(gh release create "v$VERSION"' scripts/publish-release.sh || {
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

CARGO_METADATA="$(cargo metadata --manifest-path src-tauri/Cargo.toml --offline --locked --no-deps --format-version 1)"
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
