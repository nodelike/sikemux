// Writes the generated protocol files, or with --check fails when any of them is stale.
import { mkdir, readdir, readFile, writeFile } from "node:fs/promises";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import prettier from "prettier";

import {
  bundle,
  client,
  collect,
  HEADER,
  openapi,
  reachable,
  rust,
  SchemaError,
  swift,
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

/**
 * The definitions the phone's Rust names, generated into sikemux-wire. Every other one goes in
 * sikemux-core, so a change to it leaves the phone's native code, and its over-the-air updates,
 * alone. Those fixtures go in fixtures-core, which the core's own contract test reads.
 */
const PHONE_RUST = new Set([
  "AppVersion",
  "ChannelVersions",
  "DeviceKey",
  "JoinTicket",
  "MinimumVersions",
  "Network",
  "Relay",
  "RevokeReason",
]);
/**
 * The definitions the iPhone app reads and sends, generated into Swift with everything they refer
 * to. Its own names for other things, in SWIFT_RENAMED, take the Wire prefix here.
 */
const PHONE_SWIFT = [
  "Account",
  "AccountDeletionRequest",
  "Challenge",
  "Device",
  "DeviceList",
  "DeviceRegistration",
  "ApiError",
  "JoinRequest",
  "JoinTicket",
  "LiveAck",
  "LiveCloseCode",
  "LiveHello",
  "LivePong",
  "LiveServerMessage",
  "Network",
  "PendingHost",
  "PushTokenRegistration",
  "PushTokenState",
  "ShareClientMessage",
  "ShareCloseCode",
  "ShareServerMessage",
  "StashAdd",
  "StashItem",
  "StashList",
  "StashSeen",
  "StashUpdate",
];
const SWIFT_RENAMED = new Set([
  "Account",
  "AccountStatus",
  "Device",
  "Network",
  "ShareAsk",
  "ShareFrame",
]);
/** Definitions sikemux-wire carries, which the app already has in Wire.generated.swift. */
const SWIFT_FROM_THE_WIRE = new Set(["RevokeReason"]);
/** What the share extension sends as well as the app, in a file both compile; the app's own file leaves them out. */
const SHARED_SWIFT = ["StashTarget", "StashAdd", "ApiError"];
const FROM_THE_WIRE = "\nuse sikemux_wire::accounts::protocol::*;\n";
const CORE_ONLY = new Map([
  ["share.json", { module: "share.rs", imports: FROM_THE_WIRE }],
  [
    "host.json",
    { module: "host.rs", imports: `\nuse super::protocol::*;${FROM_THE_WIRE}` },
  ],
  ["hooks.json", { module: "hooks.rs", imports: FROM_THE_WIRE }],
  ["vault.json", { module: "vault.rs", imports: FROM_THE_WIRE }],
]);

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
    [join(protocol, "generated/client.ts")]: client(routes),
    [join(repo, "mobile/ios/Sikemux/Account/Protocol.generated.swift")]: swift(
      definitions,
      {
        roots: PHONE_SWIFT,
        renamed: SWIFT_RENAMED,
        provided: new Set([
          ...SWIFT_FROM_THE_WIRE,
          ...reachable(definitions, SHARED_SWIFT),
          "DeviceKey",
        ]),
      },
    ),
    [join(repo, "mobile/ios/Shared/Stash/Stash.generated.swift")]: swift(
      definitions,
      { roots: SHARED_SWIFT },
    ),
    [join(repo, "crates/sikemux-wire/src/accounts/protocol.rs")]: rust(
      definitions,
      {
        include: (definition) => PHONE_RUST.has(definition.name),
      },
    ),
    [join(repo, "crates/sikemux-corekit/src/accounts/protocol.rs")]: rust(
      definitions,
      {
        include: (definition) =>
          !PHONE_RUST.has(definition.name) && !CORE_ONLY.has(definition.file),
        imports: FROM_THE_WIRE,
      },
    ),
  };
  for (const [file, { module, imports }] of CORE_ONLY)
    outputs[join(repo, "crates/sikemux-corekit/src/accounts", module)] = rust(
      definitions,
      {
        include: (definition) => definition.file === file,
        imports,
      },
    );
} catch (error) {
  if (!(error instanceof SchemaError)) throw error;
  console.error(`The protocol schema is not valid: ${error.message}`);
  process.exit(1);
}

const stale = [];
for (const [path, raw] of Object.entries(outputs)) {
  const text = /\.(rs|swift)$/.test(path) ? raw : await format(raw, path);
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
