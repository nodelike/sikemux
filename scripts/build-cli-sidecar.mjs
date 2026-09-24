#!/usr/bin/env node

import { chmodSync, copyFileSync, mkdirSync, rmSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

import { smokeBrowserSidecar } from "./smoke-browser-sidecar.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const tauriDir = join(root, "src-tauri");
const binariesDir = join(tauriDir, "binaries");
const args = process.argv.slice(2);
const sidecars = ["sikemux-editor", "sikemux-tools-mcp"];

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
  const cargoArgs = ["build", "--locked", "--release"];
  for (const name of sidecars) cargoArgs.push("--bin", name);
  cargoArgs.push("--manifest-path", join(tauriDir, "Cargo.toml"));
  if (explicitTarget) cargoArgs.push("--target", target);
  run("cargo", cargoArgs);
  const releaseDir = explicitTarget
    ? join(tauriDir, "target", target, "release")
    : join(tauriDir, "target", "release");
  return (name) => join(releaseDir, executableName(name, target));
}

const requestedTarget = option("--target") || hostTriple();
if (requestedTarget.includes("apple-darwin")) {
  run("bash", [join(root, "scripts", "icons.sh")]);
}
mkdirSync(binariesDir, { recursive: true });

const suffix = requestedTarget.includes("windows") ? ".exe" : "";
const shipped = new Map();
if (requestedTarget === "universal-apple-darwin") {
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
  const browser = shipped.get("sikemux-tools-mcp");
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
