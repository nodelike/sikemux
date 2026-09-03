#!/usr/bin/env node

import { spawn, spawnSync } from "node:child_process";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const signalExitCodes = {
  SIGHUP: 129,
  SIGINT: 130,
  SIGQUIT: 131,
  SIGTERM: 143,
};

function delay(milliseconds) {
  return new Promise((resolveDelay) => setTimeout(resolveDelay, milliseconds));
}

export async function readRecordedPid(path) {
  const value = await readFile(path, "utf8").catch(() => "");
  if (!/^\d+\s*$/.test(value)) return null;
  const pid = Number.parseInt(value, 10);
  return Number.isSafeInteger(pid) && pid > 1 ? pid : null;
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

export async function stopRecordedBrowser(pidFile) {
  const pid = await readRecordedPid(pidFile);
  if (pid === null) return false;
  signalProcessTree(pid, "SIGTERM");
  await delay(250);
  signalProcessTree(pid, "SIGKILL");
  await rm(pidFile, { force: true });
  return true;
}

export async function runDevDesktop() {
  const temporaryRoot = await mkdtemp(join(tmpdir(), "sikemux-dev-"));
  const browserPidFile = join(temporaryRoot, "browser.pid");
  const command = process.platform === "win32" ? "pnpm.cmd" : "pnpm";
  const child = spawn(
    command,
    ["exec", "tauri", "dev", "--config", "src-tauri/tauri.dev.conf.json"],
    {
      cwd: root,
      detached: process.platform !== "win32",
      env: { ...process.env, SIKEMUX_BROWSER_PID_FILE: browserPidFile },
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
    await stopRecordedBrowser(browserPidFile);
    await rm(temporaryRoot, { force: true, recursive: true });
  }

  if (requestedExitCode !== null) return requestedExitCode;
  if (outcome.code !== null) return outcome.code;
  return outcome.signal ? 1 : 0;
}

const isMain = process.argv[1]
  ? resolve(process.argv[1]) === fileURLToPath(import.meta.url)
  : false;

if (isMain) process.exitCode = await runDevDesktop();
