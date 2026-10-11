import { createHmac, randomBytes } from "node:crypto";
import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";

import { sweepClerk } from "../src/account/clerk-sweeper.ts";
import type { Database } from "../src/db.ts";
import { RateLimiter } from "../src/limits.ts";
import {
  caller,
  emptyTables,
  migratedDatabase,
  registered,
} from "./accounts.ts";
import { FakeClerk } from "./clerk.ts";
import { log, testApp } from "./support.ts";
import { sessionToken } from "./tokens.ts";

const SECRET_BYTES = randomBytes(24);
const SECRET = `whsec_${SECRET_BYTES.toString("base64")}`;

let database: Database;
let drop: () => Promise<void>;
let clerk: FakeClerk;
let app: ReturnType<typeof testApp>;

beforeAll(async () => {
  ({ database, drop } = await migratedDatabase());
});

beforeEach(async () => {
  await emptyTables(database);
  clerk = new FakeClerk();
  app = testApp(database, new RateLimiter(), { clerk, webhookSecret: SECRET });
});

afterAll(() => drop());

function signed(
  payload: unknown,
  {
    id = `msg_${randomBytes(8).toString("hex")}`,
    timestamp = Math.floor(Date.now() / 1000),
    key = SECRET_BYTES,
  } = {},
) {
  const body = JSON.stringify(payload);
  const signature = createHmac("sha256", key)
    .update(`${id}.${timestamp}.${body}`)
    .digest("base64");
  return {
    body,
    headers: {
      "content-type": "application/json",
      "svix-id": id,
      "svix-timestamp": String(timestamp),
      "svix-signature": `v1,${signature}`,
    },
  };
}

function deliver(request: { body: string; headers: Record<string, string> }) {
  return app.request("/v1/webhooks/clerk", { method: "POST", ...request });
}

const userDeleted = (id: string) => ({
  type: "user.deleted",
  object: "event",
  data: { id, object: "user", deleted: true },
});

async function state() {
  const users = await database.pool.query(
    "select id, deleted_at is not null as deleted, clerk_deleted_at is not null as gone_from_clerk, purge_after is not null as purging from users order by id",
  );
  const devices = await database.pool.query("select key from devices");
  const tombstones = await database.pool.query(
    "select reason from removed_devices order by key",
  );
  const log = await database.pool.query(
    "select type, reason from events where user_id = 'user_a' order by id",
  );
  return {
    users: users.rows,
    devices: devices.rows,
    tombstones: tombstones.rows,
    events: log.rows,
  };
}

describe("Clerk's webhook", () => {
  it("cleans up a user deleted in Clerk's dashboard exactly as deleting the account does", async () => {
    await registered(app, "user_a", "host");
    await registered(app, "user_a", "client");
    const response = await deliver(signed(userDeleted("user_a")));
    expect(response.status).toBe(204);
    const fromWebhook = await state();
    expect(clerk.deleted).toEqual([]);

    await emptyTables(database);
    await registered(app, "user_a", "host");
    await registered(app, "user_a", "client");
    const deleting = await caller(app)(
      "/v1/account",
      await sessionToken("user_a", { claims: { fva: [0, -1] } }),
      { method: "DELETE" },
    );
    expect(deleting.status).toBe(202);
    await database.pool.query("update users set clerk_retry_at = now()");
    await sweepClerk(database.db, clerk, log);
    expect(await state()).toEqual(fromWebhook);
    expect(fromWebhook.users).toEqual([
      { id: "user_a", deleted: true, gone_from_clerk: true, purging: true },
    ]);
  });

  it("finishes a deletion that was waiting on Clerk", async () => {
    await registered(app, "user_a", "host");
    clerk.failing = true;
    await caller(app)(
      "/v1/account",
      await sessionToken("user_a", { claims: { fva: [0, -1] } }),
      { method: "DELETE" },
    );
    await deliver(signed(userDeleted("user_a")));
    expect((await state()).users).toMatchObject([{ gone_from_clerk: true }]);
  });

  it("refuses a missing or wrong signature", async () => {
    const good = signed(userDeleted("user_a"));
    const { "svix-signature": _, ...unsigned } = good.headers;
    expect((await deliver({ ...good, headers: unsigned })).status).toBe(401);
    const forged = signed(userDeleted("user_a"), { key: randomBytes(24) });
    expect((await deliver(forged)).status).toBe(401);
    const tampered = { ...good, body: good.body.replace("user_a", "user_b") };
    expect((await deliver(tampered)).status).toBe(401);
    expect((await state()).users).toEqual([]);
  });

  it("refuses a timestamp more than five minutes off", async () => {
    const old = signed(userDeleted("user_a"), {
      timestamp: Math.floor(Date.now() / 1000) - 6 * 60,
    });
    expect((await deliver(old)).status).toBe(401);
    const future = signed(userDeleted("user_a"), {
      timestamp: Math.floor(Date.now() / 1000) + 6 * 60,
    });
    expect((await deliver(future)).status).toBe(401);
  });

  it("does nothing for a message id it has already handled", async () => {
    const message = signed(userDeleted("user_a"), { id: "msg_once" });
    expect((await deliver(message)).status).toBe(204);
    await database.pool.query("truncate users cascade");
    expect((await deliver(message)).status).toBe(204);
    expect((await state()).users).toEqual([]);
  });

  it("accepts and ignores other events", async () => {
    const response = await deliver(
      signed({ type: "user.updated", data: { id: "user_a" } }),
    );
    expect(response.status).toBe(204);
    expect((await state()).users).toEqual([]);
  });

  it("accepts nothing when the API has no webhook secret", async () => {
    app = testApp(database, new RateLimiter(), { clerk, webhookSecret: null });
    const response = await deliver(signed(userDeleted("user_a")));
    expect(response.status).toBe(503);
    expect((await state()).users).toEqual([]);
  });
});
