import { createPrivateKey, randomBytes } from "node:crypto";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { validator, type Definitions } from "@sikemux/protocol";
import pg from "pg";
import { inject } from "vitest";

import joinVector from "../../protocol/vectors/join.json" with { type: "json" };
import { createApp, type Services } from "../src/app.ts";
import { HookHosts } from "../src/hooks/hosts.ts";
import { RelayAccess } from "../src/relay/access.ts";
import { openDatabase, type Database } from "../src/db.ts";
import type { JoinSigner } from "../src/join/signer.ts";
import { RateLimiter } from "../src/limits.ts";
import { createLogger } from "../src/log.ts";
import { readNetwork } from "../src/network/network.ts";
import { NO_PLACES } from "../src/places.ts";
import { APP_ORIGIN, verifier } from "./tokens.ts";

export { APP_ORIGIN };

export const log = createLogger("silent");
export const protocol = validator();

/** Reads a response body, failing the test unless it matches the named definition. */
export async function body<Name extends keyof Definitions & string>(
  response: Response,
  name: Name,
): Promise<Definitions[Name]> {
  const result = protocol.validate(name, await response.json());
  if (!result.ok)
    throw new Error(`The body is not a ${name}: ${result.problems.join("; ")}`);
  return result.value;
}

/** A new, empty database for one test file, dropped when the file is done. */
export async function freshDatabase(): Promise<{
  url: string;
  drop(): Promise<void>;
}> {
  const adminUrl = inject("adminUrl");
  const name = `sikemux_test_${randomBytes(6).toString("hex")}`;
  const admin = new pg.Client({ connectionString: adminUrl });
  await admin.connect();
  await admin.query(`create database ${name}`);
  await admin.end();
  const url = new URL(adminUrl);
  url.pathname = `/${name}`;
  return {
    url: url.toString(),
    async drop() {
      const client = new pg.Client({ connectionString: adminUrl });
      await client.connect();
      await client.query(`drop database if exists ${name} with (force)`);
      await client.end();
    },
  };
}

/** Signs tickets with the join vector's throwaway key. */
export const joinSigner: JoinSigner = {
  keyId: joinVector.ticket.keyId,
  privateKey: createPrivateKey(joinVector.privateKeyPem),
  publicKey: joinVector.publicKey,
  file: "vectors/join.json",
  created: false,
};

/** A file holding the join vector's key, for configurations that need one. */
export function joinKeyFile(): string {
  const file = join(
    mkdtempSync(join(tmpdir(), "sikemux-join-")),
    "join-signing-key.pem",
  );
  writeFileSync(file, joinVector.privateKeyPem);
  return file;
}

export function testApp(
  database: Database,
  limiter = new RateLimiter(),
  services: Partial<Services> = {},
) {
  return createApp({
    database,
    log,
    appOrigin: APP_ORIGIN,
    apiOrigin: "https://api.sikemux.com",
    verifier,
    limiter,
    relayAccess: new RelayAccess(database.db),
    clerk: null,
    appleSignIn: null,
    webhookSecret: null,
    network: readNetwork({}, []),
    push: { app: "production", allowSandbox: false },
    join: joinSigner,
    places: NO_PLACES,
    relayAccessToken: null,
    stash: null,
    hookHosts: new HookHosts(),
    ...services,
  });
}

/** A database whose server is not there, for checking how the API copes without one. */
export function unreachableDatabase(): Database {
  return openDatabase("postgresql://nobody@127.0.0.1:1/nothing", log);
}
