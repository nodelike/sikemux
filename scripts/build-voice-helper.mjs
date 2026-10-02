#!/usr/bin/env node

import { chmodSync, copyFileSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const tauriDir = join(root, "src-tauri");
const packageDir = join(tauriDir, "voice");
const args = process.argv.slice(2);
const name = "sikemux-voice";

function fail(message) {
  console.error(`Voice helper build failed: ${message}`);
  process.exit(1);
}

function run(command, commandArgs, options = {}) {
  const result = spawnSync(command, commandArgs, {
    cwd: root,
    encoding: "utf8",
    stdio: options.capture ? ["ignore", "pipe", "inherit"] : "inherit",
  });
  if (result.error) fail(`${command}: ${result.error.message}`);
  if (result.status !== 0)
    fail(`${command} exited with status ${result.status}`);
  return result.stdout?.trim() ?? "";
}

function option(flag) {
  const exact = args.indexOf(flag);
  if (exact >= 0) return args[exact + 1] ?? "";
  return (
    args.find((arg) => arg.startsWith(`${flag}=`))?.slice(flag.length + 1) ?? ""
  );
}

function hostTriple() {
  const details = run("rustc", ["-vV"], { capture: true });
  const host = details.match(/^host:\s*(\S+)$/m)?.[1];
  if (!host) fail("could not determine the Rust host target");
  return host;
}

const target = option("--target") || hostTriple();
if (!target.includes("apple-darwin")) {
  console.log(`- ${name} skipped: dictation is macOS only`);
  process.exit(0);
}

const archs = {
  "aarch64-apple-darwin": ["arm64"],
  "x86_64-apple-darwin": ["x86_64"],
  "universal-apple-darwin": ["arm64", "x86_64"],
}[target];
if (!archs) fail(`unsupported target ${target}`);

const swiftArgs = [
  "build",
  "-c",
  "release",
  "--package-path",
  packageDir,
  // The model runs on the Neural Engine, so optimising the Swift for size
  // makes the helper about 15% smaller without slowing transcription.
  "-Xswiftc",
  "-Osize",
  ...archs.flatMap((arch) => ["--arch", arch]),
];
run("swift", swiftArgs);
const built = join(
  run("swift", [...swiftArgs, "--show-bin-path"], { capture: true }),
  name,
);

const destination = args.includes("--dev")
  ? join(tauriDir, "target", "debug", name)
  : join(tauriDir, "binaries", `${name}-${target}`);
mkdirSync(dirname(destination), { recursive: true });
copyFileSync(built, destination);
chmodSync(destination, 0o755);
run("strip", ["-x", destination]);

const loadCommands = run("otool", ["-l", destination], { capture: true });
const toolchainPaths = [
  ...loadCommands.matchAll(/^\s+path (\/Applications\/\S+) \(offset \d+\)$/gm),
].map((match) => match[1]);
for (const path of new Set(toolchainPaths))
  run("install_name_tool", ["-delete_rpath", path, destination]);

// A release publishes the helper beside the app instead of bundling it, so it is
// signed here the way the bundler signs what it ships: hardened runtime, and the
// app's microphone entitlement.
if (!args.includes("--dev")) {
  const identity = process.env.APPLE_SIGNING_IDENTITY || "-";
  run("codesign", [
    "--force",
    "--identifier",
    "com.nodelike.sikemux.voice",
    "--options",
    "runtime",
    "--entitlements",
    join(tauriDir, "Entitlements.plist"),
    ...(identity === "-" ? [] : ["--timestamp"]),
    "--sign",
    identity,
    destination,
  ]);
}

if (target === hostTriple() || target === "universal-apple-darwin") {
  const version = run(destination, ["--version"], { capture: true });
  if (!version.startsWith(name))
    fail(`unexpected --version output: ${version}`);
}

console.log(`✓ ${name} ready: ${destination.slice(root.length + 1)}`);
