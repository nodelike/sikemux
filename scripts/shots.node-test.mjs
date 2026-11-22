import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { guardCheckout } from "./lib/shots-checkout.mjs";

// On 22 Nov a teammate ran the shots.mjs of its own branch, behind the fix, and the throwaway core
// it started opened Google's Antigravity sign-in page in the person's browser.
const BEHIND = "cb54989c916b80fa05becb607c8b02efbd2b51a8";

function git(cwd, ...args) {
  return execFileSync("git", args, { cwd, encoding: "utf8" });
}

function oldShooter() {
  const path = `${BEHIND}:scripts/shots.mjs`;
  try {
    return git(import.meta.dirname, "show", path);
  } catch {
    git(import.meta.dirname, "fetch", "--quiet", "--depth=1", "origin", BEHIND);
    return git(import.meta.dirname, "show", path);
  }
}

test(
  "a shots.mjs from a checkout behind the fix builds nothing",
  { skip: process.platform === "win32" },
  () => {
    const root = mkdtempSync(join(tmpdir(), "shots-behind-"));
    try {
      const behind = join(root, "behind");
      mkdirSync(join(behind, "scripts"), { recursive: true });
      writeFileSync(join(behind, "scripts/shots.mjs"), oldShooter());
      const identity = [
        "-c",
        "user.name=shots",
        "-c",
        "user.email=shots@localhost",
      ];
      git(behind, "init", "--quiet");
      git(behind, "add", "scripts/shots.mjs");
      git(behind, ...identity, "commit", "--quiet", "-m", "behind");

      const home = join(root, "home");
      const checkout = join(
        home,
        "Library/Caches/com.nodelike.sikemux/shots/checkout",
      );
      mkdirSync(join(checkout, ".."), { recursive: true });
      git(
        import.meta.dirname,
        "clone",
        "--quiet",
        "--shared",
        "--no-checkout",
        behind,
        checkout,
      );
      guardCheckout(checkout);

      const stand = join(root, "bin");
      const built = join(root, "built");
      mkdirSync(stand);
      writeFileSync(join(stand, "cargo"), `#!/bin/sh\ntouch "${built}"\n`, {
        mode: 0o755,
      });

      const shot = spawnSync(
        "node",
        [join(behind, "scripts/shots.mjs"), "mac", "HEAD", "context-handoff"],
        {
          cwd: behind,
          encoding: "utf8",
          env: {
            ...process.env,
            HOME: home,
            PATH: `${stand}:${process.env.PATH}`,
          },
        },
      );

      assert.notEqual(shot.status, 0);
      assert.match(shot.stderr, /older than main's/);
      assert.equal(existsSync(built), false, "the old shooter reached a build");
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  },
);
