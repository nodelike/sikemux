import { afterAll, beforeAll, beforeEach, expect, it } from "vitest";

import type { Database } from "../src/db.ts";
import { vaultUses } from "../src/vault/state.ts";
import { emptyTables, migratedDatabase } from "./accounts.ts";

const USER = "user_signins";
const ENTRY = "a".repeat(32);
const HOST = "b".repeat(64);

let database: Database;
let drop: () => Promise<void>;

beforeAll(async () => {
  ({ database, drop } = await migratedDatabase());
});

beforeEach(async () => {
  await emptyTables(database);
  await database.db.insertInto("users").values({ id: USER }).execute();
});

afterAll(async () => {
  await drop();
});

const NOW = Date.now();

function minutesAgo(minutes: number): string {
  return new Date(NOW - minutes * 60_000).toISOString();
}

async function reported(actor: string, count: number, minutes: number) {
  await database.db
    .insertInto("vault_uses")
    .values({
      user_id: USER,
      entry_id: ENTRY,
      plugin: "sikemux.vercel",
      host_key: HOST,
      actor,
      count,
      first_at: minutesAgo(minutes),
      last_at: minutesAgo(minutes),
    })
    .execute();
}

it("lists a sign-in a host reports every minute once, with its uses added up", async () => {
  await reported("Sikemux", 1, 3);
  await reported("Sikemux", 2, 2);
  await reported("Sikemux", 1, 1);
  await reported("Otter", 1, 10);

  const { uses } = await vaultUses(database.db, USER);

  expect(
    uses.map((use) => [use.actor, use.count, use.firstAt, use.lastAt]),
  ).toEqual([
    ["Sikemux", 4, minutesAgo(3), minutesAgo(1)],
    ["Otter", 1, minutesAgo(10), minutesAgo(10)],
  ]);
});
