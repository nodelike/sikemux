#!/usr/bin/env node

import {
  chmodSync,
  copyFileSync,
  mkdirSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

import { appMetadata } from "./app-metadata.mjs";
import { smokeBrowserSidecar } from "./smoke-browser-sidecar.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const packaging = join(root, "packaging");
const binariesDir = join(packaging, "binaries");
const sidecars = ["sikemux-host"];

function fail(message) {
  console.error(`Sidecar build failed: ${message}`);
  process.exit(1);
}

function run(command, commandArgs, options = {}) {
  const result = spawnSync(command, commandArgs, {
    cwd: root,
    encoding: "utf8",
    stdio: options.capture ? "pipe" : "inherit",
  });
  if (result.error) fail(`${command}: ${result.error.message}`);
  if (result.status !== 0) {
    if (options.capture && result.stderr) process.stderr.write(result.stderr);
    fail(`${command} exited with status ${result.status}`);
  }
  return result.stdout?.trim() ?? "";
}

function hostTriple() {
  const details = run("rustc", ["-vV"], { capture: true });
  const host = details.match(/^host:\s*(\S+)$/m)?.[1];
  if (!host) fail("could not determine the Rust host target");
  return host;
}

function executableName(name, target) {
  return target.includes("windows") ? `${name}.exe` : name;
}

function buildFor(target) {
  const cargoArgs = ["build", "--locked", "--release", "-p", "sikemux-host"];
  for (const name of sidecars) cargoArgs.push("--bin", name);
  run("cargo", cargoArgs);
  return (name) =>
    join(root, "target", "release", executableName(name, target));
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
    join(root, "crates", "sikemux-core", "resources", "sikemux_pi_tools.ts"),
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
const shipped = new Map();
// The Release workflow builds the core on a machine of its own, alongside the helpers.
const prebuiltCore = process.env.PREBUILT_CORE;
if (prebuiltCore) {
  const destination = join(
    binariesDir,
    `sikemux-host-${requestedTarget}${suffix}`,
  );
  copyFileSync(prebuiltCore, destination);
  chmodSync(destination, 0o755);
  shipped.set("sikemux-host", destination);
} else {
  const built = buildFor(requestedTarget);
  for (const name of sidecars) {
    const destination = join(
      binariesDir,
      `${name}-${requestedTarget}${suffix}`,
    );
    copyFileSync(built(name), destination);
    shipped.set(name, destination);
  }
}

for (const destination of shipped.values()) {
  if (!requestedTarget.includes("windows")) chmodSync(destination, 0o755);
}

const browser = shipped.get("sikemux-host");
try {
  await smokeBrowserSidecar(browser);
} catch (error) {
  rmSync(browser, { force: true });
  fail(`browser sidecar smoke: ${error.message}`);
}

for (const [name, destination] of shipped) {
  console.log(`✓ ${name} ready: ${destination.slice(root.length + 1)}`);
}

if (requestedTarget.includes("apple-darwin")) {
  const app = bundleCore(shipped.get("sikemux-host"));
  console.log(`✓ Sikemux Core.app ready: ${app.slice(root.length + 1)}`);
}
