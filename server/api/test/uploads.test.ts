import { randomBytes } from "node:crypto";
import { mkdtempSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, beforeAll, beforeEach, expect, it } from "vitest";

import type { Database } from "../src/db.ts";
import { removeDevice } from "../src/devices/removal.ts";
import { bodySha256, requestMessage } from "../src/devices/signature.ts";
import { StashFiles } from "../src/stash/files.ts";
import {
  emptyTables,
  migratedDatabase,
  registered,
  signText,
  type TestDevice,
} from "./accounts.ts";
import { testApp } from "./support.ts";

const ARJUN = "user_arjun";

let database: Database;
let drop: () => Promise<void>;
let app: ReturnType<typeof testApp>;
let dir: string;
let host: TestDevice;

beforeAll(async () => {
  ({ database, drop } = await migratedDatabase());
});

beforeEach(async () => {
  await emptyTables(database);
  dir = mkdtempSync(join(tmpdir(), "sikemux-uploads-"));
  const stash = new StashFiles({ dir, key: randomBytes(32) });
  app = testApp(database, undefined, { stash });
  host = await registered(app, ARJUN, "host");
});

afterAll(() => drop());

/** Sends a file as the host, which is removed from the account once the server starts reading it. */
function uploadWhileRemoved(
  method: string,
  path: string,
  headers: Record<string, string>,
) {
  const file = new TextEncoder().encode("a file");
  const sha = bodySha256(file);
  const time = String(Math.floor(Date.now() / 1000));
  const message = requestMessage("host", method, path, time, sha);
  return app.request(path, {
    method,
    body: new ReadableStream(
      {
        async pull(body) {
          await database.db.transaction().execute((trx) =>
            removeDevice(trx, {
              userId: ARJUN,
              key: host.key,
              reason: "removed",
              actor: "system",
            }),
          );
          body.enqueue(file);
          body.close();
        },
      },
      { highWaterMark: 0 },
    ),
    duplex: "half",
    headers: {
      ...headers,
      authorization: `SikemuxHost ${host.key}.${time}.${signText(host, message)}`,
      "x-body-sha256": sha,
    },
  } as RequestInit);
}

const keptFiles = () =>
  readdirSync(dir, { recursive: true, withFileTypes: true }).filter((entry) =>
    entry.isFile(),
  ).length;

it("stashes nothing from a host removed while it uploads", async () => {
  const response = await uploadWhileRemoved("POST", "/v1/stash", {
    "content-type": "text/plain",
    "x-stash-name": "notes.txt",
  });

  expect(response.status).toBe(401);
  const items = await database.db
    .selectFrom("stash_items")
    .select("id")
    .execute();
  expect(items).toEqual([]);
  expect(keptFiles()).toBe(0);
});

it("keeps no mod's file from a host removed while it uploads", async () => {
  await database.db
    .insertInto("vaults")
    .values({
      user_id: ARJUN,
      recovery_key: "0".repeat(64),
      recovery_wrap: "d3JhcA",
      created_by: host.key,
    })
    .execute();
  const response = await uploadWhileRemoved(
    "PUT",
    `/v1/mods/${"a".repeat(32)}/files/${"b".repeat(64)}`,
    { "content-type": "application/octet-stream" },
  );

  expect(response.status).toBe(401);
  const files = await database.db
    .selectFrom("mod_files")
    .select("file_id")
    .execute();
  expect(files).toEqual([]);
  expect(keptFiles()).toBe(0);
});
