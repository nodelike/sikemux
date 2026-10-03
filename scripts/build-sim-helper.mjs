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

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const tauriDir = join(root, "src-tauri");
const simDir = join(tauriDir, "sim");
const idbDir = join(simDir, "idb");
const buildDir = join(simDir, ".build");
const args = process.argv.slice(2);
const name = "sikemux-sim";
const deploymentTarget = "15.0";

const frameworks = join(simDir, "Frameworks");

function fail(message) {
  console.error(`Simulator helper build failed: ${message}`);
  process.exit(1);
}

function run(command, commandArgs, options = {}) {
  const result = spawnSync(command, commandArgs, {
    cwd: options.cwd ?? root,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
    env: { ...process.env, ...options.env },
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

function filesIn(dir) {
  return readdirSync(dir, { recursive: true, withFileTypes: true })
    .filter((entry) => entry.isFile())
    .map((entry) => join(entry.parentPath, entry.name))
    .sort();
}

function fingerprint(paths) {
  const hash = createHash("sha256");
  for (const path of paths)
    for (const file of statSync(path).isDirectory() ? filesIn(path) : [path]) {
      hash.update(relative(root, file));
      hash.update(readFileSync(file));
    }
  return hash.digest("hex");
}

const target = option("--target") || hostTriple();
if (!target.includes("apple-darwin")) {
  console.log(`- ${name} skipped: the iOS Simulator is macOS only`);
  process.exit(0);
}

const archs = {
  "aarch64-apple-darwin": ["arm64"],
  "x86_64-apple-darwin": ["x86_64"],
  "universal-apple-darwin": ["arm64", "x86_64"],
}[target];
if (!archs) fail(`unsupported target ${target}`);

const developerDir = run("xcode-select", ["-p"], { capture: true });

// FBControlCore's Swift and Objective-C halves import each other, which one
// SwiftPM target cannot hold, so XcodeGen and xcodebuild build it for both
// architectures into the xcframework Package.swift links. It is rebuilt only
// when its sources or its spec change.
function buildFBControlCore() {
  const spec = join(simDir, "FBControlCore.yml");
  const xcframework = join(frameworks, "FBControlCore.xcframework");
  const stamp = join(frameworks, "FBControlCore.fingerprint");
  const current = fingerprint([join(idbDir, "FBControlCore"), spec]);
  if (existsSync(stamp) && readFileSync(stamp, "utf8") === current) return;
  if (spawnSync("xcodegen", ["--version"]).status !== 0)
    fail("XcodeGen is needed to build FBControlCore: brew install xcodegen");
  console.log("- building FBControlCore (once per change to it)");
  const xcode = join(buildDir, "xcode");
  rmSync(xcode, { recursive: true, force: true });
  rmSync(frameworks, { recursive: true, force: true });
  run("xcodegen", ["generate", "--spec", spec, "--quiet"], { cwd: simDir });
  run("xcodebuild", [
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
    "ARCHS=arm64 x86_64",
    "ONLY_ACTIVE_ARCH=NO",
    "ENABLE_USER_SCRIPT_SANDBOXING=NO",
    `SYMROOT=${join(xcode, "products")}`,
    `OBJROOT=${join(xcode, "objects")}`,
    "build",
  ]);
  // Built and linked by the same compiler, so the framework carries a binary Swift module and no interface.
  run("xcodebuild", [
    "-create-xcframework",
    "-allow-internal-distribution",
    "-framework",
    join(xcode, "products", "Release", "FBControlCore.framework"),
    "-output",
    xcframework,
  ]);
  writeFileSync(stamp, current);
}

// One build per architecture: building both in one `swift build` switches to
// Xcode's build system, which links FBControlCore into the executable twice.
function buildHelper(arch) {
  const triple = `${arch}-apple-macosx${deploymentTarget}`;
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
const built = archs.map(buildHelper);
const destination = args.includes("--dev")
  ? join(tauriDir, "target", "debug", name)
  : join(tauriDir, "binaries", `${name}-${target}`);
mkdirSync(dirname(destination), { recursive: true });
if (built.length === 1) copyFileSync(built[0], destination);
else run("lipo", ["-create", ...built, "-output", destination]);
chmodSync(destination, 0o755);
run("strip", ["-x", destination]);

const loadCommands = run("otool", ["-l", destination], { capture: true });
const toolchainPaths = [
  ...loadCommands.matchAll(/^\s+path (\/Applications\/\S+) \(offset \d+\)$/gm),
].map((match) => match[1]);
for (const path of new Set(toolchainPaths))
  run("install_name_tool", ["-delete_rpath", path, destination]);

// Like the voice helper, a release publishes this beside the app, so it is
// signed here the way the bundler signs what it ships.
if (!args.includes("--dev")) {
  const identity = process.env.APPLE_SIGNING_IDENTITY || "-";
  run("codesign", [
    "--force",
    "--identifier",
    "com.nodelike.sikemux.sim",
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

const size = (statSync(destination).size / 1024 / 1024).toFixed(1);
console.log(
  `✓ ${name} ready: ${destination.slice(root.length + 1)} (${size} MB)`,
);
