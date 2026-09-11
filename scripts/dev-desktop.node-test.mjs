import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  readRecordedPid,
  signalProcessTree,
  stopRecordedBrowser,
  stopProcessTree,
} from "./dev-desktop.mjs";

test("readRecordedPid accepts only a safe process id", async () => {
  const directory = await mkdtemp(join(tmpdir(), "sikemux-dev-pid-test-"));
  const pidFile = join(directory, "browser.pid");
  try {
    await writeFile(pidFile, "4242\n");
    assert.equal(await readRecordedPid(pidFile), 4242);
    await writeFile(pidFile, "not-a-pid\n");
    assert.equal(await readRecordedPid(pidFile), null);
    await writeFile(pidFile, "1\n");
    assert.equal(await readRecordedPid(pidFile), null);
  } finally {
    await rm(directory, { force: true, recursive: true });
  }
});

test(
  "stopRecordedBrowser terminates the recorded process group",
  { skip: process.platform === "win32" },
  async () => {
    const directory = await mkdtemp(join(tmpdir(), "sikemux-dev-tree-test-"));
    const pidFile = join(directory, "browser.pid");
    const child = spawn(
      "/bin/sh",
      ["-c", "trap '' TERM; while :; do sleep 1; done"],
      { detached: true, stdio: "ignore" },
    );
    const exited = new Promise((resolveExit) =>
      child.once("exit", resolveExit),
    );
    try {
      await writeFile(pidFile, `${child.pid}\n`);
      assert.equal(await stopRecordedBrowser(pidFile), true);
      await exited;
      assert.equal(await readRecordedPid(pidFile), null);
      assert.equal(signalProcessTree(child.pid, "SIGKILL"), false);
    } finally {
      signalProcessTree(child.pid, "SIGKILL");
      await rm(directory, { force: true, recursive: true });
    }
  },
);

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
