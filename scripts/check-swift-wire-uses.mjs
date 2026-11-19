#!/usr/bin/env node
// Fails when the Rust no longer gives a wire type that the iPhone app or the notch
// island still names in its own Swift. Compiling those apps takes minutes; this
// compares the generated Swift's type names on main with this checkout's.
// `--base <commit>` compares with that commit instead of main.

import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const GENERATED = [
  "helpers/notch/Sources/NotchKit/Wire.generated.swift",
  "mobile/ios/Sikemux/Core/Wire.generated.swift",
  "mobile/ios/Shared/Notify/Card.generated.swift",
  "mobile/ios/Shared/Glance/Glance.generated.swift",
];
const GENERATOR_INPUTS = [
  "crates",
  "scripts/generate-wire-types.mjs",
  "helpers/notch/wire.json",
  "mobile/app/wire.json",
];

const git = (...args) =>
  execFileSync("git", args, { cwd: root, encoding: "utf8" }).trim();
const at = process.argv.indexOf("--base");
const base =
  at === -1
    ? git("merge-base", "HEAD", "refs/heads/main")
    : process.argv[at + 1];
execFileSync("node", [join(root, "scripts/generate-wire-types.mjs")], {
  stdio: "inherit",
});
if (base === git("rev-parse", "HEAD")) process.exit(0);

function declared(checkout) {
  const names = new Set();
  for (const file of GENERATED) {
    const text = readFileSync(join(checkout, file), "utf8");
    for (const [, name] of text.matchAll(
      /^\s*(?:public\s+|indirect\s+)*(?:struct|enum|class|typealias|protocol)\s+(\w+)/gm,
    ))
      names.add(name);
  }
  return names;
}

const before = mkdtempSync(join(tmpdir(), "swift-wire-base-"));
let gone;
try {
  const archive = execFileSync(
    "git",
    ["archive", base, "--", ...GENERATOR_INPUTS],
    {
      cwd: root,
      maxBuffer: 1 << 30,
    },
  );
  execFileSync("tar", ["-x", "-C", before], { input: archive });
  execFileSync("node", [join(before, "scripts/generate-wire-types.mjs")], {
    stdio: "ignore",
  });
  const now = declared(root);
  gone = [...declared(before)].filter((name) => !now.has(name));
} finally {
  rmSync(before, { recursive: true, force: true });
}

const uses = gone.flatMap((name) => {
  try {
    return git(
      "grep",
      "--untracked",
      "-nw",
      name,
      "--",
      "mobile/ios/*.swift",
      "helpers/notch/*.swift",
      ":!*.generated.swift",
    )
      .split("\n")
      .map((line) => `${line.slice(0, 160)}   (${name})`);
  } catch {
    return [];
  }
});
if (uses.length > 0) {
  console.error(
    `The Swift these name a wire type the Rust no longer has; change it with the Rust:\n${uses.join("\n")}`,
  );
  process.exit(1);
}
