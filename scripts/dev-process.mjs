import { spawnSync } from "node:child_process";

// A Sikemux terminal names its own app, project and agent in these variables.
// Sikemux Dev started from one must not take them as its own.
const KEPT_OVERRIDES = new Set(["SIKEMUX_CORE_SOCKET", "SIKEMUX_SIDECAR_PATH"]);
// What a Claude Code session sets for the programs it runs, as LAUNCHING_SESSION in
// sikemux-process's vars.rs names them: Sikemux Dev started from one would hand
// them to every agent, turning transcript saving off and pointing at its Claude home.
const CLAUDE_SESSION = [
  "CLAUDECODE",
  "CLAUDE_CODE_CHILD_SESSION",
  "CLAUDE_CODE_ENTRYPOINT",
  "CLAUDE_CODE_EXECPATH",
  "CLAUDE_CODE_MESSAGING_SOCKET",
  "CLAUDE_CODE_MESSAGING_TOKEN",
  "CLAUDE_CODE_SESSION_ID",
  "CLAUDE_EFFORT",
  "CLAUDE_PID",
  "CLAUDE_CONFIG_DIR",
];

export function withoutTerminalSession(env) {
  const clean = { ...env };
  for (const key of Object.keys(clean)) {
    if (
      (key === "SIKEMUX" || key.startsWith("SIKEMUX_")) &&
      !KEPT_OVERRIDES.has(key)
    )
      delete clean[key];
  }
  for (const key of CLAUDE_SESSION) delete clean[key];
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
