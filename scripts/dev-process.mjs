import { spawnSync } from "node:child_process";
import { userInfo } from "node:os";
import { join } from "node:path";

/** Where a core listens, as `sikemux-core` names it. */
export function coreSocket(name) {
  return join(userInfo().homedir, ".config", "sikemux", name);
}

// A Sikemux terminal names its own app, project and agent in these variables.
// Sikemux Dev started from one must not take them as its own.
const KEPT_OVERRIDES = new Set(["SIKEMUX_CORE_SOCKET", "SIKEMUX_SIDECAR_PATH"]);
// The installed app's core, which a Sikemux terminal may name as its socket.
const INSTALLED_CORE = coreSocket("core.sock");

export function withoutTerminalSession(env) {
  const clean = { ...env };
  for (const key of Object.keys(clean)) {
    if (
      (key === "SIKEMUX" || key.startsWith("SIKEMUX_")) &&
      !KEPT_OVERRIDES.has(key)
    )
      delete clean[key];
  }
  if (clean.SIKEMUX_CORE_SOCKET === INSTALLED_CORE)
    delete clean.SIKEMUX_CORE_SOCKET;
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
