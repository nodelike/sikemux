#!/usr/bin/env node

import {
  chmodSync,
  copyFileSync,
  mkdirSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

import { smokeBrowserSidecar } from "./smoke-browser-sidecar.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const tauriDir = join(root, "src-tauri");
const binariesDir = join(tauriDir, "binaries");
const args = process.argv.slice(2);
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

function option(name) {
  const exact = args.indexOf(name);
  if (exact >= 0) {
    if (!args[exact + 1] || args[exact + 1].startsWith("--"))
      fail(`${name} requires a value`);
    return args[exact + 1];
  }
  const prefix = `${name}=`;
  return args.find((arg) => arg.startsWith(prefix))?.slice(prefix.length) ?? "";
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

function buildFor(target, explicitTarget) {
  const cargoArgs = ["build", "--locked", "--release", "-p", "sikemux-host"];
  for (const name of sidecars) cargoArgs.push("--bin", name);
  cargoArgs.push("--manifest-path", join(tauriDir, "Cargo.toml"));
  if (explicitTarget) cargoArgs.push("--target", target);
  run("cargo", cargoArgs);
  const releaseDir = explicitTarget
    ? join(tauriDir, "target", target, "release")
    : join(tauriDir, "target", "release");
  return (name) => join(releaseDir, executableName(name, target));
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
  const config = JSON.parse(
    readFileSync(join(tauriDir, "tauri.conf.json"), "utf8"),
  );
  const appInfo = JSON.parse(
    run(
      "plutil",
      ["-convert", "json", "-o", "-", join(tauriDir, "Info.plist")],
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
    LSMinimumSystemVersion: "11.0",
    LSUIElement: true,
  };
  for (const [key, value] of Object.entries(appInfo)) {
    if (key.endsWith("UsageDescription")) info[key] = value;
  }
  const plist = join(contents, "Info.plist");
  writeFileSync(plist, JSON.stringify(info));
  run("plutil", ["-convert", "xml1", plist]);
  copyFileSync(
    join(tauriDir, "icons", "icon.icns"),
    join(contents, "Resources", "sikemux.icns"),
  );
  copyFileSync(
    join(tauriDir, "resources", "sikemux_pi_tools.ts"),
    join(contents, "Resources", "sikemux_pi_tools.ts"),
  );
  const executable = join(contents, "MacOS", "sikemux-host");
  copyFileSync(binary, executable);
  chmodSync(executable, 0o755);
  mkdirSync(join(staging, "MacOS"));
  symlinkSync(
    "../Helpers/Sikemux Core.app/Contents/MacOS/sikemux-host",
    join(staging, "MacOS", "sikemux-host"),
  );
  const identity = process.env.APPLE_SIGNING_IDENTITY || "-";
  run("codesign", [
    "--force",
    "--options",
    "runtime",
    ...(identity === "-" ? [] : ["--timestamp"]),
    "--entitlements",
    join(tauriDir, "Entitlements.plist"),
    "--sign",
    identity,
    app,
  ]);
  return app;
}

const requestedTarget = option("--target") || hostTriple();
if (requestedTarget.includes("apple-darwin")) {
  run("bash", [join(root, "scripts", "icons.sh")]);
}
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
} else if (requestedTarget === "universal-apple-darwin") {
  const arm = buildFor("aarch64-apple-darwin", true);
  const intel = buildFor("x86_64-apple-darwin", true);
  for (const name of sidecars) {
    const destination = join(binariesDir, `${name}-${requestedTarget}`);
    run("lipo", ["-create", "-output", destination, arm(name), intel(name)]);
    shipped.set(name, destination);
  }
} else {
  const built = buildFor(requestedTarget, Boolean(option("--target")));
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

// A cross-built sidecar cannot be started here, so the smoke only runs for a
// binary this machine can execute.
const runnable =
  requestedTarget === hostTriple() ||
  requestedTarget === "universal-apple-darwin";
if (runnable && !args.includes("--skip-smoke")) {
  const browser = shipped.get("sikemux-host");
  try {
    await smokeBrowserSidecar(browser);
  } catch (error) {
    rmSync(browser, { force: true });
    fail(`browser sidecar smoke: ${error.message}`);
  }
}

for (const [name, destination] of shipped) {
  console.log(`✓ ${name} ready: ${destination.slice(root.length + 1)}`);
}

if (requestedTarget.includes("apple-darwin")) {
  const app = bundleCore(shipped.get("sikemux-host"));
  console.log(`✓ Sikemux Core.app ready: ${app.slice(root.length + 1)}`);
}
