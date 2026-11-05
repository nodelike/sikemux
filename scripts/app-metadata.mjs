#!/usr/bin/env node
// The app's name, identifiers, version, oldest macOS and updater key, read from the Cargo.toml
// files without cargo, so the signing jobs need nothing but node.
//   node scripts/app-metadata.mjs name     one field
//   node scripts/app-metadata.mjs          all of them, as JSON
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

export const MANIFEST = fileURLToPath(
  new URL("../app/Cargo.toml", import.meta.url),
);

function table(text, name) {
  const lines = text.split("\n");
  const start = lines.indexOf(`[${name}]`);
  if (start < 0) throw new Error(`${MANIFEST} has no [${name}]`);
  const values = {};
  for (const line of lines.slice(start + 1)) {
    if (line.startsWith("[")) break;
    const pair = line.match(/^([A-Za-z0-9_-]+) = "([^"]*)"$/);
    if (pair) values[pair[1]] = pair[2];
  }
  return values;
}

const WORKSPACE = fileURLToPath(new URL("../Cargo.toml", import.meta.url));

export function appMetadata() {
  return {
    ...table(readFileSync(MANIFEST, "utf8"), "package.metadata.sikemux"),
    version: table(readFileSync(WORKSPACE, "utf8"), "workspace.package")
      .version,
  };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const [field] = process.argv.slice(2);
  if (!field) {
    process.stdout.write(`${JSON.stringify(appMetadata())}\n`);
    process.exit(0);
  }
  const value = appMetadata()[field];
  if (value === undefined) {
    console.error(`app-metadata: no ${field ?? "field"} in ${MANIFEST}`);
    process.exit(1);
  }
  process.stdout.write(`${value}\n`);
}
