import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import test from "node:test";

import {
  signalProcessTree,
  stopProcessTree,
  withoutTerminalSession,
} from "./dev-desktop.mjs";

test(
  "launcher cleanup stops descendants after their parent exits",
  { skip: process.platform === "win32" },
  async () => {
    const parent = spawn("/bin/sh", ["-c", "sleep 60 & exit 0"], {
      detached: true,
      stdio: "ignore",
    });
    await new Promise((resolveExit) => parent.once("exit", resolveExit));
    try {
      process.kill(-parent.pid, 0);
      await stopProcessTree(parent.pid);
      assert.equal(signalProcessTree(parent.pid, "SIGTERM"), false);
    } finally {
      signalProcessTree(parent.pid, "SIGKILL");
    }
  },
);

test("Sikemux Dev started from a Sikemux terminal drops that terminal's identity", () => {
  const env = withoutTerminalSession({
    PATH: "/usr/bin",
    TERM_PROGRAM: "Sikemux",
    TERM_PROGRAM_VERSION: "0.4.2",
    SIKEMUX: "1",
    SIKEMUX_BIN_PATH: "/Applications/Sikemux.app/Contents/MacOS/sikemux-editor",
    SIKEMUX_CLI_ENDPOINT: "/Users/me/.config/sikemux/cli.json",
    SIKEMUX_AGENT_ID: "agent-1",
    SIKEMUX_CORE_SOCKET: "/tmp/core.sock",
    SIKEMUX_SIDECAR_PATH: "/tmp/sidecar",
  });
  assert.deepEqual(env, {
    PATH: "/usr/bin",
    SIKEMUX_CORE_SOCKET: "/tmp/core.sock",
    SIKEMUX_SIDECAR_PATH: "/tmp/sidecar",
  });
});
