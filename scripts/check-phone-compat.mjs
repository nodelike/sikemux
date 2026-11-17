#!/usr/bin/env node
// Fails when this checkout's core would break a phone build that is in the app stores, live or
// in review, as mobile/store-versions.json lists them: the phone must still be served at a
// protocol version, every call it makes must still exist, and the JSON it sends and reads must
// still fit. Also fails when a `phone-compat:` mark names a version no longer listed there. A mark
// may go on to name the JSON fields its code puts right for that version, which the check then passes.

import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const STORES = "mobile/store-versions.json";
const MARK = /phone-compat:\s*(\d+\.\d+\.\d+)((?:[ \t]+[A-Za-z]\w*)*)/g;

const git = (...args) =>
  execFileSync("git", args, {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 1 << 28,
  });
const shapes = (checkout) =>
  JSON.parse(
    execFileSync(
      process.execPath,
      [join(root, "scripts/generate-wire-types.mjs"), "--shapes", checkout],
      { encoding: "utf8", maxBuffer: 1 << 28 },
    ),
  );

const { versions } = JSON.parse(readFileSync(join(root, STORES), "utf8"));
const problems = [];

const fixed = new Map(versions.map((version) => [version, new Set()]));
let marked = "";
try {
  marked = git(
    "grep",
    "-nI",
    "--untracked",
    "-E",
    "phone-compat:[[:space:]]*[0-9]+[.][0-9]+[.][0-9]+",
  );
} catch (error) {
  if (error.status !== 1) throw error;
}
for (const [, file, line, text] of marked.matchAll(/^(.*?):(\d+):(.*)$/gm)) {
  for (const [, version, names] of text.matchAll(MARK))
    if (fixed.has(version))
      for (const name of names.split(/\s+/).filter(Boolean))
        fixed.get(version).add(name);
    else
      problems.push(
        `${file}:${line} is kept for phone ${version}, which ${STORES} no longer lists: delete it`,
      );
}

/** The phone build of `version` as its tag holds it, fetched when a shallow clone lacks it. */
function tagged(version) {
  const tag = `mobile-v${version}`;
  try {
    git("rev-parse", "-q", "--verify", `refs/tags/${tag}`);
  } catch {
    git("fetch", "-q", "--depth=1", "origin", "tag", tag);
  }
  const dir = mkdtempSync(join(tmpdir(), "phone-compat-"));
  execFileSync(
    "sh",
    [
      "-c",
      'git archive "$1" crates mobile/app/wire.json helpers/notch/wire.json | tar -x -C "$2"',
      "sh",
      tag,
      dir,
    ],
    { cwd: root },
  );
  return dir;
}

const main = shapes(root);
const strip = (value) =>
  JSON.parse(
    JSON.stringify(value, (key, inner) => (key === "docs" ? undefined : inner)),
  );

for (const version of versions) {
  const dir = tagged(version);
  let phone;
  try {
    phone = shapes(dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
  const report = (where, message) => {
    if (!fixed.get(version).has(where.split(".").at(-1)))
      problems.push(`phone ${version}: ${where}: ${message}`);
  };

  // As sikemux-serve's agreed_version: the newest version both speak, if the core still serves it.
  const agreed = Math.min(phone.protocol.version, main.protocol.version);
  if (agreed < Math.max(phone.protocol.oldest, main.protocol.oldest))
    report(
      "protocol",
      `it speaks ${phone.protocol.oldest} to ${phone.protocol.version}, the core serves ${main.protocol.oldest} to ${main.protocol.version}`,
    );

  const compared = new Set();
  const optional = (field) =>
    field.type.kind === "option" || field.mayBeAbsent || field.defaulted;

  /** Whether JSON of type `ours` (main's) and `theirs` (the phone's) still fit, as `way` carries it. */
  function type(theirs, ours, way, where) {
    if (theirs.kind === "option" && ours.kind === "option")
      return type(theirs.of, ours.of, way, where);
    if (theirs.kind === "option" && way === "sent")
      return report(
        where,
        "the phone may send null, which the core no longer takes",
      );
    if (theirs.kind === "option") return type(theirs.of, ours, way, where);
    if (ours.kind === "option" && way === "read")
      return report(
        where,
        "the core may send null, which the phone cannot read",
      );
    if (ours.kind === "option") return type(theirs, ours.of, way, where);
    if (theirs.kind === "json" && way === "read") return;
    if (theirs.kind !== ours.kind)
      return report(where, `was ${theirs.kind}, is ${ours.kind}`);
    if (theirs.kind === "named")
      return declaration(theirs.name, ours.name, way, where);
    if (theirs.kind === "tuple") {
      if (theirs.of.length !== ours.of.length)
        return report(where, "the tuple changed length");
      theirs.of.forEach((of, index) =>
        type(of, ours.of[index], way, `${where}.${index}`),
      );
    } else if (theirs.of) type(theirs.of, ours.of, way, `${where}[]`);
  }

  function fields(theirs, ours, way, where, { defaulted, denyUnknown } = {}) {
    for (const old of theirs) {
      const now = ours.find((field) => field.name === old.name);
      const at = `${where}.${old.name}`;
      if (way === "read") {
        if (!now) {
          if (!optional(old)) report(at, "the core no longer sends it");
          continue;
        }
        if (!optional(old) && (now.mayBeAbsent || now.defaulted))
          report(at, "the core may leave it out, which the phone cannot read");
      } else {
        if (!now) {
          if (denyUnknown) report(at, "the core refuses it now");
          continue;
        }
        if (old.mayBeAbsent && !optional(now))
          report(
            at,
            "the phone may leave it out, which the core no longer takes",
          );
      }
      type(old.type, now.type, way, at);
    }
    if (way === "sent")
      for (const now of ours)
        if (
          !theirs.some((field) => field.name === now.name) &&
          !optional(now) &&
          !defaulted
        )
          report(
            `${where}.${now.name}`,
            "the core needs it, and the phone never sends it",
          );
  }

  /** The variants of one side that the other must know: what the core sends, or what the phone sends. */
  function variants(theirs, ours, way, where, compare) {
    const [from, to] = way === "read" ? [ours, theirs] : [theirs, ours];
    for (const variant of from.variants) {
      const match = to.variants.find((other) => other.name === variant.name);
      if (match)
        compare(...(way === "read" ? [match, variant] : [variant, match]));
      else if (way === "sent" && !to.variants.some((other) => other.other))
        report(`${where}::${variant.name}`, "the core no longer takes it");
    }
  }

  /** A declaration the parser cannot read on either side, compared as its Rust. */
  function source(theirs, ours, way, where, ourName) {
    if (theirs.rust !== ours.rust)
      report(where, `${ourName}'s serde settings changed`);
    for (const [member, rust] of Object.entries(theirs.members))
      if (ours.members[member] !== undefined && ours.members[member] !== rust)
        report(`${where}::${member}`, "changed");
      else if (ours.members[member] === undefined && way === "sent")
        report(`${where}::${member}`, "the core no longer has it");
    if (way === "sent")
      for (const member of Object.keys(ours.members))
        if (theirs.members[member] === undefined)
          report(
            `${where}::${member}`,
            "the core has it, and the phone does not",
          );
    for (const name of theirs.uses)
      if (main.declarations[name] && phone.declarations[name])
        declaration(name, name, way, `${where} ${name}`);
      else if (ours.uses.includes(name)) report(where, `${name} is gone`);
  }

  function declaration(theirName, ourName, way, where) {
    const key = `${way} ${theirName} ${ourName}`;
    if (compared.has(key)) return;
    compared.add(key);
    const theirs = phone.declarations[theirName];
    const ours = main.declarations[ourName];
    if (theirs.kind === "rust" || ours.kind === "rust")
      return source(theirs.source, ours.source, way, where, ourName);
    if (theirs.kind !== ours.kind)
      return report(
        where,
        `${theirName} was a ${theirs.kind}, ${ourName} is a ${ours.kind}`,
      );
    switch (theirs.kind) {
      case "struct":
        return fields(theirs.fields, ours.fields, way, where, ours);
      case "tagged":
        if (theirs.tag !== ours.tag)
          return report(where, "its tag field changed");
        return variants(theirs, ours, way, where, (old, now) =>
          fields(old.fields, now.fields, way, `${where}::${old.name}`),
        );
      case "wrapped":
        return variants(theirs, ours, way, where, (old, now) =>
          type(old.type, now.type, way, `${where}::${old.name}`),
        );
      case "strings":
        return variants(theirs, ours, way, where, () => {});
      case "untagged":
        if (JSON.stringify(strip(theirs)) !== JSON.stringify(strip(ours)))
          report(where, `${ourName} changed`);
        return;
    }
  }

  for (const call of phone.calls) {
    const now = main.calls.find((other) => other.method === call.method);
    if (!now) {
      report(call.method, "the core no longer has this call");
      continue;
    }
    type(call.takes, now.takes, "sent", `${call.method}(params)`);
    type(call.gives, now.gives, "read", `${call.method} answer`);
  }
  for (const [way, roots] of [
    ["read", phone.read],
    ["sent", phone.sent],
  ])
    for (const name of roots)
      if (main.declarations[name]) declaration(name, name, way, name);
      else report(name, "the core no longer has it");
}

if (problems.length) {
  console.error(
    `A phone build in the stores would break, or code kept for one is stale. Keep what a listed build needs, marked \`phone-compat: <version>\`, until ${STORES} drops that version.\n`,
  );
  for (const problem of problems) console.error(`  ${problem}`);
  process.exit(1);
}
console.log(`Phones ${versions.join(", ")} still fit this core.`);
