#!/usr/bin/env node

import { spawn, spawnSync } from "node:child_process";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");

const signalExitCodes = {
  SIGHUP: 129,
  SIGINT: 130,
  SIGQUIT: 131,
  SIGTERM: 143,
};

// A Sikemux terminal names its own app, project and agent in these variables.
// Sikemux Dev started from one must not take them as its own.
const KEPT_OVERRIDES = new Set(["SIKEMUX_CORE_SOCKET", "SIKEMUX_SIDECAR_PATH"]);

export function withoutTerminalSession(env) {
  const clean = { ...env };
  for (const key of Object.keys(clean)) {
    if (
      (key === "SIKEMUX" || key.startsWith("SIKEMUX_")) &&
      !KEPT_OVERRIDES.has(key)
    )
      delete clean[key];
  }
  if (clean.TERM_PROGRAM === "Sikemux") {
    delete clean.TERM_PROGRAM;
    delete clean.TERM_PROGRAM_VERSION;
  }
  return clean;
}

function delay(milliseconds) {
  return new Promise((resolveDelay) => setTimeout(resolveDelay, milliseconds));
}

export function signalProcessTree(pid, signal) {
  if (!Number.isSafeInteger(pid) || pid <= 1) return false;
  if (process.platform === "win32") {
    const args = ["/PID", String(pid), "/T"];
    if (signal === "SIGKILL") args.push("/F");
    const result = spawnSync("taskkill", args, {
      stdio: "ignore",
      windowsHide: true,
    });
    return result.status === 0;
  }
  try {
    process.kill(-pid, signal);
    return true;
  } catch (error) {
    if (error?.code === "ESRCH") return false;
    throw error;
  }
}

export async function stopProcessTree(pid) {
  if (!pid || !signalProcessTree(pid, "SIGTERM")) return;
  await delay(250);
  signalProcessTree(pid, "SIGKILL");
}

export function findRunningDevProcess(projectRoot = root) {
  if (process.platform === "win32") return null;
  const listing = spawnSync("ps", ["-axo", "pid=,command="], {
    encoding: "utf8",
  });
  if (listing.status !== 0) return null;
  for (const line of listing.stdout.split("\n")) {
    const match = line.trim().match(/^(\d+)\s+(.+)$/);
    if (!match) continue;
    const pid = Number(match[1]);
    const executable = match[2];
    const absolute = join(
      projectRoot,
      "src-tauri",
      "target",
      "debug",
      "sikemux",
    );
    if (executable === absolute) return pid;
    if (executable !== "target/debug/sikemux") continue;
    const cwd =
      process.platform === "darwin"
        ? spawnSync("lsof", ["-a", "-p", String(pid), "-d", "cwd", "-Fn"], {
            encoding: "utf8",
          })
        : spawnSync("readlink", [`/proc/${pid}/cwd`], { encoding: "utf8" });
    const directories =
      cwd.stdout
        ?.split("\n")
        .map((value) =>
          process.platform === "darwin" ? value.slice(1) : value.trim(),
        ) ?? [];
    if (directories.includes(join(projectRoot, "src-tauri"))) return pid;
  }
  return null;
}

export async function runDevDesktop() {
  const existingPid = findRunningDevProcess();
  if (existingPid !== null) {
    console.error(
      `Sikemux Dev is already running from this checkout (PID ${existingPid}). Quit that Dev instance before running make dev again. Production Sikemux can stay open.`,
    );
    return 1;
  }
  const startedAt = Date.now();
  const command = process.platform === "win32" ? "pnpm.cmd" : "pnpm";
  const child = spawn(
    command,
    ["exec", "tauri", "dev", "--config", "src-tauri/tauri.dev.conf.json"],
    {
      cwd: root,
      detached: process.platform !== "win32",
      env: withoutTerminalSession(process.env),
      stdio: "inherit",
      windowsHide: false,
    },
  );

  let requestedExitCode = null;
  let forceTimer = null;
  const handlers = new Map();
  for (const [signal, exitCode] of Object.entries(signalExitCodes)) {
    const handler = () => {
      if (requestedExitCode !== null) return;
      requestedExitCode = exitCode;
      if (child.pid) signalProcessTree(child.pid, signal);
      forceTimer = setTimeout(() => {
        if (child.pid) signalProcessTree(child.pid, "SIGKILL");
      }, 5_000);
      forceTimer.unref();
    };
    handlers.set(signal, handler);
    process.on(signal, handler);
  }

  let outcome;
  try {
    outcome = await new Promise((resolveExit, rejectExit) => {
      child.once("error", rejectExit);
      child.once("exit", (code, signal) => resolveExit({ code, signal }));
    });
  } finally {
    if (forceTimer) clearTimeout(forceTimer);
    for (const [signal, handler] of handlers) process.off(signal, handler);
    await stopProcessTree(child.pid);
  }

  if (requestedExitCode !== null) return requestedExitCode;
  if (outcome.code === 0 && Date.now() - startedAt < 10_000) {
    console.error(
      "Sikemux Dev exited immediately. Check for another running Dev instance; a duplicate launch can exit without opening a window.",
    );
    return 1;
  }
  if (outcome.code !== null) return outcome.code;
  return outcome.signal ? 1 : 0;
}

const isMain = process.argv[1]
  ? resolve(process.argv[1]) === fileURLToPath(import.meta.url)
  : false;

if (isMain) process.exitCode = await runDevDesktop();
