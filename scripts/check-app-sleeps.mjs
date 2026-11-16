#!/usr/bin/env node
// Fails when a file in app/src waits on a clock more often than scripts/app-sleeps.json allows.
// The app reacts to events; a sleep or timer that waits for something to be ready is a race.
// A wait that is the point, such as a backoff or a debounce, raises its file's count with --write.

import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, relative, resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const BASELINE = join(root, "scripts/app-sleeps.json");
const WAIT = /\b(?:thread::sleep|time::sleep|Timer::after|timer)\(/g;

function* rustFiles(dir) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) yield* rustFiles(path);
    else if (entry.name.endsWith(".rs")) yield path;
  }
}

const counts = {};
for (const path of rustFiles(join(root, "app/src"))) {
  const found = readFileSync(path, "utf8").match(WAIT)?.length ?? 0;
  if (found) counts[relative(root, path)] = found;
}
const sorted = Object.fromEntries(
  Object.entries(counts).sort(([a], [b]) => a.localeCompare(b)),
);

if (process.argv.includes("--write")) {
  writeFileSync(BASELINE, `${JSON.stringify(sorted, null, 2)}\n`);
  process.exit(0);
}

const allowed = JSON.parse(readFileSync(BASELINE, "utf8"));
const over = Object.entries(sorted).filter(
  ([path, found]) => found > (allowed[path] ?? 0),
);
for (const [path, found] of over) {
  console.error(
    `${path} waits on a clock ${found} times, ${allowed[path] ?? 0} allowed`,
  );
}
if (over.length) {
  console.error(
    "Wait for the event instead: a channel, a subscription or a task's result. When the wait itself is the point, run node scripts/check-app-sleeps.mjs --write and say why in the commit.",
  );
  process.exit(1);
}
