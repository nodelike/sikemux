#!/usr/bin/env node

import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { createHash } from "node:crypto";
import { dirname, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const scriptPath = fileURLToPath(import.meta.url);
const root = resolve(dirname(scriptPath), "..");
const browserDir = join(root, "browser");
const tauriDir = join(root, "src-tauri");
const binariesDir = join(tauriDir, "binaries");
const runtimeDir = join(tauriDir, "browser-runtime");
const args = process.argv.slice(2);

function fail(message) {
  console.error(`Browser sidecar build failed: ${message}`);
  process.exit(1);
}

function run(command, commandArgs, options = {}) {
  const result = spawnSync(command, commandArgs, {
    cwd: options.cwd ?? root,
    env: { ...process.env, ...(options.env ?? {}) },
    encoding: "utf8",
    stdio: options.capture ? "pipe" : "inherit",
  });
  if (result.error || result.status !== 0) {
    for (const path of options.removeOnFailure ?? []) {
      rmSync(path, { force: true });
    }
  }
  if (result.error) fail(`${command}: ${result.error.message}`);
  if (result.status !== 0)
    fail(`${command} exited with status ${result.status}`);
  return result.stdout?.trim() ?? "";
}

function option(name) {
  const index = args.indexOf(name);
  if (index >= 0) return args[index + 1] ?? "";
  return (
    args.find((arg) => arg.startsWith(`${name}=`))?.slice(name.length + 1) ?? ""
  );
}

function hostTriple() {
  const details = run("rustc", ["-vV"], { capture: true });
  return (
    details.match(/^host:\s*(\S+)$/m)?.[1] ??
    fail("could not determine Rust host target")
  );
}

function findBrowserRuntime(directory) {
  if (!existsSync(directory)) return null;
  const pending = [directory];
  while (pending.length) {
    const current = pending.pop();
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      const path = join(current, entry.name);
      if (entry.isDirectory()) pending.push(path);
      else if (
        [
          "Chromium",
          "chrome",
          "chrome.exe",
          "Google Chrome for Testing",
        ].includes(entry.name)
      )
        return path;
    }
  }
  return null;
}

const target = option("--target") || hostTriple();
if (target === "universal-apple-darwin") {
  fail(
    "build browser sidecars per macOS architecture before assembling a universal app",
  );
}
if (target !== hostTriple()) {
  fail(`PyInstaller cannot cross-compile from ${hostTriple()} to ${target}`);
}

mkdirSync(binariesDir, { recursive: true });
const workDir = join(browserDir, ".pyinstaller");
const distDir = join(workDir, "dist");
const suffix = target.includes("windows") ? ".exe" : "";
const destination = join(binariesDir, `sikemux-browser-mcp-${target}${suffix}`);
const sidecarInputs = [
  scriptPath,
  join(browserDir, "sikemux_browser_mcp.py"),
  join(browserDir, "sikemux_harness.py"),
  join(browserDir, "pyproject.toml"),
  join(browserDir, "uv.lock"),
];
const needsSidecarBuild =
  !existsSync(destination) ||
  args.includes("--force") ||
  sidecarInputs.some(
    (input) => statSync(input).mtimeMs > statSync(destination).mtimeMs,
  );

run("uv", ["sync", "--project", browserDir, "--frozen"]);
if (needsSidecarBuild) {
  rmSync(workDir, { recursive: true, force: true });
  run(
    "uv",
    [
      "run",
      "--project",
      browserDir,
      "--with",
      "pyinstaller==6.16.0",
      "pyinstaller",
      "--noconfirm",
      "--clean",
      "--onefile",
      "--name",
      "sikemux-browser-mcp",
      "--distpath",
      distDir,
      "--workpath",
      join(workDir, "build"),
      "--specpath",
      workDir,
      "--collect-all",
      "browser_use",
      "--collect-all",
      "cdp_use",
      "--hidden-import",
      "mcp.server.stdio",
      "--hidden-import",
      "mcp.types",
      join(browserDir, "sikemux_browser_mcp.py"),
    ],
    { cwd: browserDir },
  );
  const source = join(distDir, `sikemux-browser-mcp${suffix}`);
  if (!existsSync(source)) fail(`missing PyInstaller output at ${source}`);
  copyFileSync(source, destination);
}
if (!target.includes("windows")) chmodSync(destination, 0o755);

const runtimeMarker = join(runtimeDir, ".sikemux-browser-runtime");
const runtimeFingerprint = createHash("sha256")
  .update(readFileSync(join(browserDir, "uv.lock")))
  .update("chromium-full")
  .digest("hex");
const runtimeIsCurrent =
  findBrowserRuntime(runtimeDir) &&
  existsSync(runtimeMarker) &&
  readFileSync(runtimeMarker, "utf8").trim() === runtimeFingerprint;
if (!runtimeIsCurrent) {
  rmSync(runtimeDir, { recursive: true, force: true });
  mkdirSync(runtimeDir, { recursive: true });
  run(
    "uv",
    [
      "run",
      "--project",
      browserDir,
      "python",
      "-m",
      "playwright",
      "install",
      "chromium",
      "--no-shell",
    ],
    {
      cwd: browserDir,
      env: { PLAYWRIGHT_BROWSERS_PATH: runtimeDir },
    },
  );
  writeFileSync(runtimeMarker, `${runtimeFingerprint}\n`);
}

const browserExecutable = findBrowserRuntime(runtimeDir);
if (!browserExecutable)
  fail("Chromium runtime installation produced no executable");
if (!args.includes("--skip-smoke")) {
  run(
    "uv",
    [
      "run",
      "--project",
      browserDir,
      "python",
      join(browserDir, "smoke_sikemux_browser_mcp.py"),
      "--sidecar",
      destination,
      "--browser",
      browserExecutable,
    ],
    { cwd: browserDir, removeOnFailure: [destination] },
  );
}

console.log(`✓ Browser sidecar ready: ${destination.slice(root.length + 1)}`);
console.log(`✓ Chromium runtime ready: ${runtimeDir.slice(root.length + 1)}`);
