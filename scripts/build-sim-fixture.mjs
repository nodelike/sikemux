#!/usr/bin/env node

// Builds the iOS app the simulator's end-to-end test drives, and prints where it went.
import { spawnSync } from "node:child_process";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const fixture = join(root, "src-tauri", "sim", "Fixture");
const derived = join(root, "src-tauri", "sim", ".build", "fixture");

function run(command, args) {
  const result = spawnSync(command, args, { cwd: fixture, stdio: "inherit" });
  if (result.error || result.status !== 0) {
    console.error(
      `Simulator fixture build failed: ${command} ${result.error?.message ?? `exited with ${result.status}`}`,
    );
    process.exit(1);
  }
}

run("xcodegen", ["generate", "--quiet"]);
run("xcodebuild", [
  "-project",
  "SimFixture.xcodeproj",
  "-scheme",
  "SimFixture",
  "-sdk",
  "iphonesimulator",
  "-destination",
  "generic/platform=iOS Simulator",
  "-configuration",
  "Debug",
  "-derivedDataPath",
  derived,
  "-quiet",
  "build",
]);
console.log(
  join(derived, "Build", "Products", "Debug-iphonesimulator", "SimFixture.app"),
);
