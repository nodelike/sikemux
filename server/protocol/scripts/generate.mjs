// Writes the generated protocol files, or with --check fails when any of them is stale.
import { mkdir, readdir, readFile, writeFile } from "node:fs/promises";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import prettier from "prettier";

import {
  bundle,
  collect,
  HEADER,
  openapi,
  rust,
  SchemaError,
  typescript,
} from "./generator.mjs";

const protocol = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repo = resolve(protocol, "../..");
const check = process.argv.includes("--check");

async function readJson(path) {
  return JSON.parse(await readFile(path, "utf8"));
}

async function format(text, path) {
  const options = await prettier.resolveConfig(path);
  return prettier.format(text, { ...options, filepath: path });
}

const schemaDir = join(protocol, "schema");
const documents = {};
for (const file of (await readdir(schemaDir))
  .filter((name) => name.endsWith(".json"))
  .sort()) {
  documents[file] = await readJson(join(schemaDir, file));
}

let definitions;
let outputs;
try {
  definitions = collect(documents);
  const routes = await readJson(join(protocol, "routes.json"));
  const schemaModule =
    HEADER +
    `export const schema = ${JSON.stringify(bundle(definitions))} as const;\n`;
  outputs = {
    [join(protocol, "generated/types.ts")]: typescript(definitions),
    [join(protocol, "generated/schema.ts")]: schemaModule,
    [join(protocol, "generated/openapi.json")]: JSON.stringify(
      openapi(definitions, routes),
    ),
    [join(repo, "src-tauri/crates/sikemux-wire/src/accounts/protocol.rs")]:
      rust(definitions),
  };
} catch (error) {
  if (!(error instanceof SchemaError)) throw error;
  console.error(`The protocol schema is not valid: ${error.message}`);
  process.exit(1);
}

const stale = [];
for (const [path, raw] of Object.entries(outputs)) {
  const text = path.endsWith(".rs") ? raw : await format(raw, path);
  const current = await readFile(path, "utf8").catch(() => null);
  if (current === text) continue;
  stale.push(relative(repo, path));
  if (!check) {
    await mkdir(dirname(path), { recursive: true });
    await writeFile(path, text);
  }
}

if (check && stale.length) {
  console.error(
    `These files do not match server/protocol/schema:\n  ${stale.join("\n  ")}\nRun pnpm protocol:generate in server/.`,
  );
  process.exit(1);
}
console.log(
  stale.length
    ? `Wrote ${stale.join(", ")}`
    : "The generated protocol files are up to date.",
);
