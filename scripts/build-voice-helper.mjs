#!/usr/bin/env node

import { join } from "node:path";
import { building, root } from "./lib/build.mjs";

const { fail, run, hostTriple, finishHelper } = building("Voice helper");

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

const destination = finishHelper(built, name, target, {
  dev: args.includes("--dev"),
});

console.log(`✓ ${name} ready: ${destination.slice(root.length + 1)}`);
