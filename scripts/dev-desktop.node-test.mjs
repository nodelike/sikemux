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
