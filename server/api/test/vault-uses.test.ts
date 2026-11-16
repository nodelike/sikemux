import { afterAll, beforeAll, beforeEach, expect, it } from "vitest";

import type { Database } from "../src/db.ts";
import { addUses } from "../src/vault/change.ts";
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

it("keeps one row for a sign-in a host reports every minute", async () => {
  const noon = new Date(NOW).setUTCHours(12, 0, 0, 0);
  const at = (minute: number) => new Date(noon + minute * 60_000).toISOString();
  const send = (minute: number) =>
    addUses(
      database.db,
      { userId: USER, hostKey: HOST },
      {
        uses: [
          {
            entryId: ENTRY,
            plugin: "sikemux.vercel",
            actor: "Sikemux",
            count: 1,
            firstAt: at(minute),
            lastAt: at(minute),
          },
        ],
      },
    );
  await send(1);
  await send(2);
  await send(3);

  const rows = await database.db
    .selectFrom("vault_uses")
    .select(["count", "first_at", "last_at"])
    .execute();

  expect(
    rows.map((row) => [
      row.count,
      row.first_at.toISOString(),
      row.last_at.toISOString(),
    ]),
  ).toEqual([[3, at(1), at(3)]]);
});
