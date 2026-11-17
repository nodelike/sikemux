#!/usr/bin/env node
// Fails when the phone's Rust (sikemux-mobile) builds with sikemux-core, sikemux-pty or
// workspace-hack, on any platform. Only what it links counts, never its tests' dependencies.

import { execFileSync } from "node:child_process";
import { resolve } from "node:path";

const PHONE = "sikemux-mobile";
const BANNED = ["sikemux-core", "sikemux-pty", "workspace-hack"];

const metadata = JSON.parse(
  execFileSync("cargo", ["metadata", "--format-version", "1", "--locked"], {
    cwd: resolve(import.meta.dirname, ".."),
    encoding: "utf8",
    maxBuffer: 1 << 28,
  }),
);
const names = new Map(metadata.packages.map((pkg) => [pkg.id, pkg.name]));
const nodes = new Map(metadata.resolve.nodes.map((node) => [node.id, node]));
const linked = (dep) => dep.dep_kinds.some((kind) => kind.kind !== "dev");

const phone = metadata.packages.find((pkg) => pkg.name === PHONE);
if (!phone) throw new Error(`${PHONE} is not in the workspace`);

const via = new Map([[phone.id, null]]);
const queue = [phone.id];
while (queue.length) {
  const id = queue.shift();
  for (const dep of nodes.get(id)?.deps ?? []) {
    if (!linked(dep) || via.has(dep.pkg)) continue;
    via.set(dep.pkg, id);
    queue.push(dep.pkg);
  }
}

const found = [...via.keys()].filter((id) => BANNED.includes(names.get(id)));
for (const id of found) {
  const path = [];
  for (let at = id; at; at = via.get(at)) path.unshift(names.get(at));
  console.error(
    `${PHONE} must not depend on ${names.get(id)}: ${path.join(" → ")}`,
  );
}
if (found.length) {
  console.error(
    "The phone links only sikemux-wire and sikemux-client; see AGENTS.md, Protocol and clients.",
  );
  process.exit(1);
}
