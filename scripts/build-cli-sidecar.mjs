#!/usr/bin/env node

import { chmodSync, copyFileSync, mkdirSync } from "node:fs";
import { basename, dirname, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const tauriDir = join(root, "src-tauri");
const binariesDir = join(tauriDir, "binaries");
const args = process.argv.slice(2);

function fail(message) {
  console.error(`CLI sidecar build failed: ${message}`);
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

function executableName(target) {
  return target.includes("windows") ? "sikemux-editor.exe" : "sikemux-editor";
}

function buildFor(target, explicitTarget) {
  const cargoArgs = [
    "build",
    "--locked",
    "--release",
    "--bin",
    "sikemux-editor",
    "--manifest-path",
    join(tauriDir, "Cargo.toml"),
  ];
  if (explicitTarget) cargoArgs.push("--target", target);
  run("cargo", cargoArgs);
  return explicitTarget
    ? join(tauriDir, "target", target, "release", executableName(target))
    : join(tauriDir, "target", "release", executableName(target));
}

const requestedTarget = option("--target") || hostTriple();
mkdirSync(binariesDir, { recursive: true });

let source;
if (requestedTarget === "universal-apple-darwin") {
  const arm = buildFor("aarch64-apple-darwin", true);
  const intel = buildFor("x86_64-apple-darwin", true);
  source = join(binariesDir, "sikemux-editor-universal-apple-darwin");
  run("lipo", ["-create", "-output", source, arm, intel]);
} else {
  const explicitTarget = Boolean(option("--target"));
  source = buildFor(requestedTarget, explicitTarget);
  const suffix = requestedTarget.includes("windows") ? ".exe" : "";
  const destination = join(
    binariesDir,
    `sikemux-editor-${requestedTarget}${suffix}`,
  );
  copyFileSync(source, destination);
  source = destination;
}

if (!requestedTarget.includes("windows")) chmodSync(source, 0o755);
console.log(
  `✓ CLI sidecar ready: ${source.slice(root.length + 1)} (${basename(source)})`,
);
