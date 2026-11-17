#!/usr/bin/env node

import {
  chmodSync,
  copyFileSync,
  mkdirSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";

import { appMetadata } from "./app-metadata.mjs";
import { smokeCore } from "./smoke-core.mjs";
import { building, root } from "./lib/build.mjs";

const { fail, run, hostTriple } = building("Core");

const packaging = join(root, "packaging");
const binariesDir = join(packaging, "binaries");

function executableName(name, target) {
  return target.includes("windows") ? `${name}.exe` : name;
}

function buildFor(target) {
  run("cargo", [
    "build",
    "--locked",
    "--release",
    "-p",
    "sikemux-host",
    "--bin",
    "sikemux-host",
  ]);
  return join(
    root,
    "target",
    "release",
    executableName("sikemux-host", target),
  );
}

// A macOS release runs the core as an app of its own, since only an app can
// carry the provisioning profile its keychain needs. Contents/MacOS links to it.
function bundleCore(binary) {
  const staging = join(binariesDir, "core");
  const app = join(staging, "Helpers", "Sikemux Core.app");
  const contents = join(app, "Contents");
  rmSync(staging, { recursive: true, force: true });
  mkdirSync(join(contents, "MacOS"), { recursive: true });
  mkdirSync(join(contents, "Resources"), { recursive: true });
  const config = appMetadata();
  const appInfo = JSON.parse(
    run(
      "plutil",
      ["-convert", "json", "-o", "-", join(packaging, "Info.plist")],
      { capture: true },
    ),
  );
  const info = {
    CFBundleIdentifier: `${config.identifier}.core`,
    CFBundleName: "Sikemux Core",
    CFBundleExecutable: "sikemux-host",
    CFBundleIconFile: "sikemux.icns",
    CFBundleInfoDictionaryVersion: "6.0",
    CFBundlePackageType: "APPL",
    CFBundleShortVersionString: config.version,
    CFBundleVersion: config.version,
    LSMinimumSystemVersion: config["minimum-macos"],
    LSUIElement: true,
  };
  for (const [key, value] of Object.entries(appInfo)) {
    if (key.endsWith("UsageDescription")) info[key] = value;
  }
  const plist = join(contents, "Info.plist");
  writeFileSync(plist, JSON.stringify(info));
  run("plutil", ["-convert", "xml1", plist]);
  copyFileSync(
    join(packaging, "icons", "build", "sikemux.icns"),
    join(contents, "Resources", "sikemux.icns"),
  );
  copyFileSync(
    join(root, "crates", "sikemux-agents", "resources", "sikemux_pi_tools.ts"),
    join(contents, "Resources", "sikemux_pi_tools.ts"),
  );
  const executable = join(contents, "MacOS", "sikemux-host");
  copyFileSync(binary, executable);
  chmodSync(executable, 0o755);
  mkdirSync(join(staging, "MacOS"));
  symlinkSync(config["core-link"], join(staging, "MacOS", "sikemux-host"));
  const identity = process.env.APPLE_SIGNING_IDENTITY || "-";
  run("codesign", [
    "--force",
    "--options",
    "runtime",
    ...(identity === "-" ? [] : ["--timestamp"]),
    "--entitlements",
    join(packaging, "Entitlements.plist"),
    "--sign",
    identity,
    app,
  ]);
  return app;
}

const requestedTarget = hostTriple();
mkdirSync(binariesDir, { recursive: true });

const suffix = requestedTarget.includes("windows") ? ".exe" : "";
const core = join(binariesDir, `sikemux-host-${requestedTarget}${suffix}`);
// The Release workflow builds the core on a machine of its own, alongside the helpers.
copyFileSync(process.env.PREBUILT_CORE || buildFor(requestedTarget), core);
if (!requestedTarget.includes("windows")) chmodSync(core, 0o755);

try {
  await smokeCore(core);
} catch (error) {
  rmSync(core, { force: true });
  fail(`core smoke: ${error.message}`);
}
console.log(`✓ sikemux-host ready: ${core.slice(root.length + 1)}`);

if (requestedTarget.includes("apple-darwin")) {
  const app = bundleCore(core);
  console.log(`✓ Sikemux Core.app ready: ${app.slice(root.length + 1)}`);
}
