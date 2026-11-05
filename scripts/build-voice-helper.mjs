#!/usr/bin/env node

import { chmodSync, copyFileSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { building, root } from "./lib/build.mjs";

const { fail, run, hostTriple, dropToolchainPaths, checkStarts } =
  building("Voice helper");

const packageDir = join(root, "helpers", "voice");
const args = process.argv.slice(2);
const name = "sikemux-voice";

const target = hostTriple();
if (!target.includes("apple-darwin")) {
  console.log(`- ${name} skipped: dictation is macOS only`);
  process.exit(0);
}

if (target !== "aarch64-apple-darwin")
  fail(`${name} needs a Mac with Apple Silicon, not ${target}`);

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
  "--arch",
  "arm64",
];
run("swift", swiftArgs);
const built = join(
  run("swift", [...swiftArgs, "--show-bin-path"], { capture: true }),
  name,
);

const destination = args.includes("--dev")
  ? join(root, "target", "debug", name)
  : join(root, "packaging", "binaries", `${name}-${target}`);
mkdirSync(dirname(destination), { recursive: true });
copyFileSync(built, destination);
chmodSync(destination, 0o755);
run("strip", ["-x", destination]);

dropToolchainPaths(destination);

// A release publishes the helper beside the app instead of bundling it. This
// ad hoc signature lets it run here; the release signs it again before the app
// that pins its hash is built.
if (!args.includes("--dev")) {
  run("bash", [join(root, "scripts", "sign-helper.sh"), destination], {
    env: { APPLE_SIGNING_IDENTITY: "-" },
  });
}

checkStarts(destination, name);

console.log(`✓ ${name} ready: ${destination.slice(root.length + 1)}`);
