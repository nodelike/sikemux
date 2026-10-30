#!/usr/bin/env node
// Cargo never deletes a build once the hashes that name it change, and every
// Cargo.lock edit or `-p` feature set changes them. This drops the copies no
// build has used lately while keeping the ones the current checkout needs.
import { readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const HOUR = 3600 * 1000;
const DAY = 24 * HOUR;
const COPIES_KEEP = 3;
// macOS records a file's read time at most once a day, so a copy read an hour
// ago can still look a day unread.
const COPY_IDLE = DAY + HOUR;
const DEPS_IDLE = 3 * DAY;
const PRUNE_EVERY = HOUR;

const args = process.argv.slice(2);
const dryRun = args.includes("--dry-run");
const target =
  args.find((arg) => !arg.startsWith("--")) ??
  join(import.meta.dirname, "..", "target");
const now = Date.now();
let freed = 0;

const stamp = join(target, ".pruned");
if (args.includes("--throttled")) {
  const last = statSync(stamp, { throwIfNoEntry: false })?.mtimeMs ?? 0;
  if (now - last < PRUNE_EVERY) process.exit(0);
  writeFileSync(stamp, "");
}

function entries(dir) {
  try {
    return readdirSync(dir, { withFileTypes: true });
  } catch {
    return [];
  }
}

function sizeOf(path) {
  const stat = statSync(path, { throwIfNoEntry: false });
  if (!stat) return 0;
  if (!stat.isDirectory()) return stat.size;
  return entries(path).reduce(
    (sum, entry) => sum + sizeOf(join(path, entry.name)),
    0,
  );
}

// A build running alongside can delete files mid-scan; count those as in use.
function lastTouched(path, field) {
  return statSync(path, { throwIfNoEntry: false })?.[field] ?? now;
}

function remove(path) {
  freed += sizeOf(path);
  if (!dryRun) rmSync(path, { recursive: true, force: true });
}

function profileDirs(dir, depth = 0) {
  const names = new Set(entries(dir).map((entry) => entry.name));
  if (names.has("deps") && names.has(".fingerprint")) return [dir];
  if (depth === 2) return [];
  return entries(dir)
    .filter((entry) => entry.isDirectory() && !entry.name.startsWith("."))
    .flatMap((entry) => profileDirs(join(dir, entry.name), depth + 1));
}

// A crate's incremental cache is named `<crate>-<hash>`; each new hash is a new copy.
function pruneIncremental(dir) {
  const byCrate = new Map();
  for (const entry of entries(dir)) {
    const crate = entry.name.slice(0, entry.name.lastIndexOf("-"));
    const path = join(dir, entry.name);
    const copies = byCrate.get(crate) ?? [];
    copies.push({ path, mtime: lastTouched(path, "mtimeMs") });
    byCrate.set(crate, copies);
  }
  for (const copies of byCrate.values()) {
    copies.sort((a, b) => b.mtime - a.mtime);
    for (const { path, mtime } of copies.slice(COPIES_KEEP)) {
      if (now - mtime > COPY_IDLE) remove(path);
    }
  }
}

// Every compile and link reads the outputs it depends on, which refreshes their
// access time, so outputs left unread for days belong to no current build.
// Workspace crates get a new `<crate>-<hash>` copy many times a day, so beyond
// the few most recently read copies of a crate, a day unread is enough.
function pruneDeps(dir) {
  const byUnit = new Map();
  for (const entry of entries(dir)) {
    const unit = entry.name.split(".")[0].replace(/^lib/, "");
    const files = byUnit.get(unit) ?? [];
    files.push(join(dir, entry.name));
    byUnit.set(unit, files);
  }
  const byCrate = new Map();
  for (const [unit, files] of byUnit) {
    const lastRead = Math.max(
      ...files.map((file) => lastTouched(file, "atimeMs")),
    );
    const crate = unit.includes("-")
      ? unit.slice(0, unit.lastIndexOf("-"))
      : unit;
    const copies = byCrate.get(crate) ?? [];
    copies.push({ files, lastRead });
    byCrate.set(crate, copies);
  }
  for (const copies of byCrate.values()) {
    copies.sort((a, b) => b.lastRead - a.lastRead);
    copies.forEach(({ files, lastRead }, index) => {
      const idle = now - lastRead;
      if (idle > DEPS_IDLE || (index >= COPIES_KEEP && idle > COPY_IDLE)) {
        files.forEach(remove);
      }
    });
  }
}

// Cargo trusts a fingerprint without checking its outputs still exist, so a
// fingerprint left behind by a removed library makes the next link fail.
function pruneFingerprints(profile) {
  const outputs = new Set(
    entries(join(profile, "deps")).map((entry) => entry.name.split(".")[0]),
  );
  const dir = join(profile, ".fingerprint");
  for (const entry of entries(dir)) {
    const hash = entry.name.slice(entry.name.lastIndexOf("-") + 1);
    const lib = entries(join(dir, entry.name)).find(
      (file) => file.name.startsWith("lib-") && !file.name.includes("."),
    );
    if (lib && !outputs.has(`lib${lib.name.slice(4)}-${hash}`)) {
      remove(join(dir, entry.name));
    }
  }
}

for (const profile of profileDirs(target)) {
  pruneIncremental(join(profile, "incremental"));
  pruneDeps(join(profile, "deps"));
  pruneFingerprints(profile);
}

const verb = dryRun ? "Would free" : "Freed";
console.log(`${verb} ${(freed / 2 ** 30).toFixed(1)} GB from ${target}`);
