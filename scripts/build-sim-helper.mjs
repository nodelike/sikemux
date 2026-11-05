#!/usr/bin/env node

import { createHash } from "node:crypto";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { building, root } from "./lib/build.mjs";

const { fail, run, hostTriple, dropToolchainPaths, checkStarts } =
  building("Simulator helper");

const simDir = join(root, "helpers", "sim");
const idbDir = join(simDir, "idb");
const buildDir = join(simDir, ".build");
const args = process.argv.slice(2);
const dev = args.includes("--dev");
const name = "sikemux-sim";
const deploymentTarget = `${
  readFileSync(join(simDir, "Package.swift"), "utf8").match(
    /\.macOS\(\.v(\d+)\)/,
  )[1]
}.0`;

const frameworks = join(simDir, "Frameworks");

// `make dev` builds the app without the simulator helper on a Mac that cannot build it.
function missing(message) {
  if (!dev) fail(message);
  console.warn(`- skipping the simulator helper: ${message}`);
  process.exit(0);
}

function filesIn(dir) {
  return readdirSync(dir, { recursive: true, withFileTypes: true })
    .filter((entry) => entry.isFile())
    .map((entry) => join(entry.parentPath, entry.name))
    .sort();
}

function fingerprint(paths, extra = []) {
  const hash = createHash("sha256");
  for (const value of extra) hash.update(`${value}\0`);
  for (const path of paths)
    for (const file of statSync(path).isDirectory() ? filesIn(path) : [path]) {
      hash.update(relative(root, file));
      hash.update(readFileSync(file));
    }
  return hash.digest("hex");
}

const target = hostTriple();
if (!target.includes("apple-darwin")) {
  console.log(`- ${name} skipped: the iOS Simulator is macOS only`);
  process.exit(0);
}

if (target !== "aarch64-apple-darwin")
  fail(`${name} needs a Mac with Apple Silicon, not ${target}`);

function canBuildSimulators(dir) {
  return existsSync(join(dir, "Platforms", "iPhoneSimulator.platform"));
}

const developerDir = [
  run("xcode-select", ["-p"], { capture: true }),
  "/Applications/Xcode.app/Contents/Developer",
].find(canBuildSimulators);
if (!developerDir)
  missing(
    "it needs Xcode; select it with `sudo xcode-select -s /Applications/Xcode.app`",
  );
const xcodebuild = spawnSync("xcodebuild", ["-version"], {
  encoding: "utf8",
  env: { ...process.env, DEVELOPER_DIR: developerDir },
});
if (xcodebuild.status !== 0)
  missing(
    `xcodebuild cannot run (${(xcodebuild.stderr || xcodebuild.error?.message || "").trim().split("\n")[0]}); open Xcode once to finish setting it up`,
  );
const xcodeVersion = xcodebuild.stdout.trim();
// FBControlCore's Swift and Objective-C halves import each other, which one
// SwiftPM target cannot hold, so XcodeGen and xcodebuild build it into the
// xcframework Package.swift links. It is rebuilt only when its sources, its
// spec or the Xcode change.
function buildFBControlCore() {
  const spec = join(simDir, "FBControlCore.yml");
  const xcframework = join(frameworks, "FBControlCore.xcframework");
  const stamp = join(frameworks, "FBControlCore.fingerprint");
  const current = fingerprint(
    [join(idbDir, "FBControlCore"), spec],
    [xcodeVersion, "arm64"],
  );
  const archive = join(frameworks, "FBControlCore.a");
  if (
    existsSync(archive) &&
    existsSync(stamp) &&
    readFileSync(stamp, "utf8") === current
  )
    return;
  if (spawnSync("xcodegen", ["--version"]).status !== 0)
    missing("it needs XcodeGen: brew install xcodegen");
  console.log("- building FBControlCore (once per change to it)");
  const xcode = join(buildDir, "xcode");
  rmSync(xcode, { recursive: true, force: true });
  rmSync(frameworks, { recursive: true, force: true });
  const env = { DEVELOPER_DIR: developerDir };
  run("xcodegen", ["generate", "--spec", spec, "--quiet"], { cwd: simDir });
  run(
    "xcodebuild",
    [
      "-project",
      join(simDir, "FBControlCore.xcodeproj"),
      "-target",
      "FBControlCore",
      "-configuration",
      "Release",
      "-sdk",
      "macosx",
      "-quiet",
      "-skipMacroValidation",
      "ARCHS=arm64",
      "ONLY_ACTIVE_ARCH=NO",
      "ENABLE_USER_SCRIPT_SANDBOXING=NO",
      `SYMROOT=${join(xcode, "products")}`,
      `OBJROOT=${join(xcode, "objects")}`,
      "build",
    ],
    { env },
  );
  // Built and linked by the same compiler, so the framework carries a binary Swift module and no interface.
  run(
    "xcodebuild",
    [
      "-create-xcframework",
      "-allow-internal-distribution",
      "-framework",
      join(xcode, "products", "Release", "FBControlCore.framework"),
      "-output",
      xcframework,
    ],
    { env },
  );
  // The same archive at a path of its own, for Package.swift to force-load.
  copyFileSync(
    join(
      xcode,
      "products",
      "Release",
      "FBControlCore.framework",
      "FBControlCore",
    ),
    archive,
  );
  writeFileSync(stamp, current);
}

function buildHelper() {
  const triple = `arm64-apple-macosx${deploymentTarget}`;
  const swiftArgs = [
    "build",
    "-c",
    "release",
    "--package-path",
    simDir,
    "--triple",
    triple,
    "-Xswiftc",
    "-Osize",
  ];
  const env = { DEVELOPER_DIR: developerDir };
  run("swift", swiftArgs, { env });
  return join(
    run("swift", [...swiftArgs, "--show-bin-path"], { capture: true, env }),
    name,
  );
}

buildFBControlCore();
const built = buildHelper();
const destination = args.includes("--dev")
  ? join(root, "target", "debug", name)
  : join(root, "packaging", "binaries", `${name}-${target}`);
mkdirSync(dirname(destination), { recursive: true });
copyFileSync(built, destination);
chmodSync(destination, 0o755);
run("strip", ["-x", destination]);

dropToolchainPaths(destination);

// Like the voice helper, signed ad hoc to run here and signed again by the release.
if (!args.includes("--dev")) {
  run("bash", [join(root, "scripts", "sign-helper.sh"), destination], {
    env: { APPLE_SIGNING_IDENTITY: "-" },
  });
}

checkStarts(destination, name);

const size = (statSync(destination).size / 1024 / 1024).toFixed(1);
console.log(
  `✓ ${name} ready: ${destination.slice(root.length + 1)} (${size} MB)`,
);
