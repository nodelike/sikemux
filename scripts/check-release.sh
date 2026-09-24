#!/usr/bin/env bash
# Credential-free, offline checks for platform release configuration and tooling.
set -euo pipefail

cd "$(dirname "$0")/.."
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

if command -v pnpm >/dev/null 2>&1; then
  PNPM=(pnpm)
else
  PNPM=(corepack pnpm)
fi

bash -n scripts/build-mac.sh scripts/release.sh scripts/icons.sh scripts/check-release.sh
node --check scripts/verify-updater-signature.mjs
node --check scripts/build-cli-sidecar.mjs
node --check scripts/smoke-browser-sidecar.mjs
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
const config = JSON.parse(fs.readFileSync("src-tauri/tauri.conf.json", "utf8"));
const macConfig = JSON.parse(fs.readFileSync("src-tauri/tauri.macos.conf.json", "utf8"));
const windowsConfig = JSON.parse(fs.readFileSync("src-tauri/tauri.windows.conf.json", "utf8"));
const sidecarConfig = JSON.parse(fs.readFileSync("src-tauri/tauri.sidecar.conf.json", "utf8"));
const macBuild = fs.readFileSync("scripts/build-mac.sh", "utf8");
const sidecarBuild = fs.readFileSync("scripts/build-cli-sidecar.mjs", "utf8");
const fail = (message) => { throw new Error(message); };

if (pkg.version !== config.version) fail("package.json and tauri.conf.json versions differ");
if (config.bundle?.createUpdaterArtifacts !== true) fail("updater artifacts are not enabled");
if (config.app?.macOSPrivateApi !== true) fail("shared macOS private API feature flag is missing");
if (JSON.stringify(macConfig.bundle?.targets) !== JSON.stringify(["app", "dmg"])) fail("macOS bundle targets must be app and dmg");
if (macConfig.bundle?.resources?.["icons/build/Assets.car"] !== "Assets.car") fail("Assets.car resource mapping is missing");
if (macConfig.bundle?.macOS?.infoPlist !== "Info.plist") fail("pre-signing Info.plist merge is not configured");
if (macConfig.bundle?.macOS?.minimumSystemVersion !== "11.0") fail("unexpected minimum macOS version");
if (macConfig.bundle?.macOS?.entitlements !== "Entitlements.plist") fail("sidecar entitlements are not configured");
if (JSON.stringify(windowsConfig.bundle?.targets) !== JSON.stringify(["nsis"])) fail("Windows bundle target must be NSIS");
if (windowsConfig.bundle?.createUpdaterArtifacts !== false) fail("unsigned Windows builds must not require updater credentials");
if (!windowsConfig.bundle?.icon?.includes("icons/icon.ico")) fail("Windows icon is not configured");
if (windowsConfig.bundle?.windows?.nsis?.installMode !== "currentUser") fail("unexpected Windows install mode");
if (JSON.stringify(sidecarConfig.bundle?.externalBin) !== JSON.stringify(["binaries/sikemux-editor", "binaries/sikemux-tools-mcp"])) fail("sidecar bundle mapping is incomplete");
if (sidecarConfig.bundle?.resources?.["resources/sikemux_pi_tools.ts"] !== "sikemux_pi_tools.ts") fail("Pi browser extension resource mapping is missing");
if (!pkg.scripts?.["build:windows"]?.includes("build:sidecar")) fail("Windows build does not build sidecars");
if (!pkg.scripts?.["build:windows"]?.includes("tauri.sidecar.conf.json")) fail("Windows build does not bundle the CLI sidecar");
if (!macBuild.includes("build-cli-sidecar.mjs")) fail("macOS build does not build the CLI sidecar");
if (!macBuild.includes("tauri.sidecar.conf.json")) fail("macOS build does not bundle the CLI sidecar");
if (!sidecarBuild.includes("sikemux-tools-mcp")) fail("sidecar build does not build the browser MCP sidecar");
if (!sidecarBuild.includes("smokeBrowserSidecar")) fail("sidecar build does not run the browser smoke test");
const endpoints = config.plugins?.updater?.endpoints;
if (!Array.isArray(endpoints) || endpoints.length !== 1 || endpoints[0] !== "https://github.com/nodelike/sikemux/releases/latest/download/latest.json") {
  fail("updater endpoint is not the expected HTTPS latest.json URL");
}
const publicKeyText = Buffer.from(config.plugins.updater.pubkey, "base64").toString("utf8").trim();
const publicLines = publicKeyText.split("\n");
if (publicLines.length !== 2 || !publicLines[0].startsWith("untrusted comment:")) fail("invalid updater public-key envelope");
const packet = Buffer.from(publicLines[1], "base64");
if (packet.length !== 42 || packet.subarray(0, 2).toString("ascii") !== "Ed") fail("invalid updater Ed25519 public key");
NODE

# Shipped clients resolve this exact URL, so the endpoint is a fixed contract.
grep -Fq 'releases/download/nightly/latest.json' src-tauri/src/updates.rs || {
  echo "nightly updater endpoint is missing" >&2
  exit 1
}
# shellcheck disable=SC2016
grep -Fq 'gh release upload nightly "$MANIFEST" --clobber' scripts/release.sh || {
  echo "nightly feed no longer repoints its latest.json" >&2
  exit 1
}
# shellcheck disable=SC2016
grep -Fq 'NIGHTLY_GH_CMD=(gh release create "v$VERSION"' scripts/release.sh || {
  echo "nightly channel does not publish an immutable versioned release" >&2
  exit 1
}
# shellcheck disable=SC2016
if grep -E 'gh release (create|upload|edit) nightly' scripts/release.sh | grep -qE '\$(DMG|TAR|SIG)'; then
  echo "nightly channel still attaches builds to the moving pointer release" >&2
  exit 1
fi
# shellcheck disable=SC2016
grep -Fq 'MANIFEST="$BUNDLE/latest.json"' scripts/release.sh || {
  echo "nightly manifest is not written beside its artifacts" >&2
  exit 1
}
grep -Fq 'pathlib.Path("latest.json").write_text' scripts/release.sh && {
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
  echo "the Release workflow no longer publishes through scripts/release.sh" >&2
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
  const root = metadata.packages.find((pkg) => pkg.manifest_path === metadata.workspace_root + "/Cargo.toml");
  if (!root) process.exit(1);
  process.stdout.write(root.version);
' <<<"$CARGO_METADATA")"
[[ "$PACKAGE_VERSION" == "$CARGO_VERSION" ]] || {
  echo "package.json and Cargo package versions differ" >&2
  exit 1
}

# Exercise the verifier against a fresh ephemeral key and prove that changing
# the signed bytes is rejected. No project signing key or network is used.
printf 'offline updater verifier fixture\n' >"$TMP/artifact.tar.gz"
CI=true "${PNPM[@]}" tauri signer generate --password '' --write-keys "$TMP/test.key" >"$TMP/generate.out" 2>"$TMP/generate.err"
"${PNPM[@]}" tauri signer sign --private-key-path "$TMP/test.key" --password '' "$TMP/artifact.tar.gz" >"$TMP/sign.out" 2>"$TMP/sign.err"
node -e '
  const fs = require("node:fs");
  fs.writeFileSync(process.argv[2], JSON.stringify({ plugins: { updater: { pubkey: fs.readFileSync(process.argv[1], "utf8").trim() } } }));
' "$TMP/test.key.pub" "$TMP/config.json"
node scripts/verify-updater-signature.mjs "$TMP/artifact.tar.gz" "$TMP/artifact.tar.gz.sig" "$TMP/config.json" >/dev/null
printf 'tampered\n' >>"$TMP/artifact.tar.gz"
if node scripts/verify-updater-signature.mjs "$TMP/artifact.tar.gz" "$TMP/artifact.tar.gz.sig" "$TMP/config.json" >"$TMP/tamper.out" 2>&1; then
  echo "updater verifier accepted a modified artifact" >&2
  exit 1
fi

echo "✓ Release tooling static checks passed"
