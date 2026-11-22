import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";

import type { Database } from "../src/db.ts";
import { bodySha256, requestMessage } from "../src/devices/signature.ts";
import { Presence } from "../src/live/hub.ts";
import {
  emptyTables,
  migratedDatabase,
  registered,
  signText,
  type TestDevice,
} from "./accounts.ts";
import { teamPlan } from "./billing.ts";
import { FakeClerk } from "./clerk.ts";
import { body, testApp } from "./support.ts";

const OWNER = "user_owner";
const ADMIN = "user_admin";
const MEMBER = "user_member";

let database: Database;
let drop: () => Promise<void>;
let app: ReturnType<typeof testApp>;
let connected: Set<string>;

beforeAll(async () => {
  ({ database, drop } = await migratedDatabase());
});

beforeEach(async () => {
  await emptyTables(database);
  await database.pool.query("truncate orgs cascade");
  await database.pool.query(
    "insert into users (id, staff) select unnest($1::text[]), true",
    [[OWNER, ADMIN, MEMBER]],
  );
  connected = new Set();
  const presence = new Presence();
  presence.attach(
    { online: (key) => connected.has(key) },
    Date.now() - 60 * 60_000,
  );
  app = testApp(database, undefined, { clerk: new FakeClerk(), presence });
});

afterAll(() => drop());

let sent = 0;

function signed(
  host: TestDevice,
  path: string,
  method = "GET",
  json?: unknown,
) {
  const text = json === undefined ? "" : JSON.stringify(json);
  const time = String(Math.floor(Date.now() / 1000) - (sent++ % 50));
  const message = requestMessage(
    "host",
    method,
    path,
    time,
    bodySha256(new TextEncoder().encode(text)),
  );
  return app.request(path, {
    method,
    headers: {
      authorization: `SikemuxHost ${host.key}.${time}.${signText(host, message)}`,
      "content-type": "application/json",
    },
    ...(json === undefined ? {} : { body: text }),
  });
}

async function lastSeen(host: TestDevice, minutesAgo: number) {
  await database.db
    .updateTable("devices")
    .set({ last_seen_at: new Date(Date.now() - minutesAgo * 60_000) })
    .where("key", "=", host.key)
    .execute();
}

/** An org whose owner's host holds the vault's key, with an admin and a plain member. */
async function vault({ lapsed = false } = {}) {
  const { id } = await database.db
    .insertInto("orgs")
    .values({ name: "Acme", created_by: OWNER })
    .returning("id")
    .executeTakeFirstOrThrow();
  await database.db
    .insertInto("org_users")
    .values([
      { org_id: id, user_id: OWNER, role: "owner" },
      { org_id: id, user_id: ADMIN, role: "admin" },
      { org_id: id, user_id: MEMBER, role: "user" },
    ])
    .execute();
  await teamPlan(database.db, id, 5);
  const owner = await registered(app, OWNER, "host");
  const admin = await registered(app, ADMIN, "host");
  const member = await registered(app, MEMBER, "host");
  expect(
    (await signed(owner, `/v1/orgs/${id}/vault`, "POST", { key: "owner" }))
      .status,
  ).toBe(201);
  if (lapsed)
    await database.db
      .updateTable("subscriptions")
      .set({ status: "lapsed" })
      .execute();
  return { orgId: id, owner, admin, member };
}

const remake = (host: TestDevice, orgId: string, from = 1) =>
  signed(host, `/v1/orgs/${orgId}/vault/remake`, "POST", {
    from,
    key: "remade",
  });

describe("making an org's vault again", () => {
  it("is not open to a plain member, who is never told who holds the key", async () => {
    const { orgId, owner, member } = await vault();
    await lastSeen(owner, 60);

    const seen = await body(
      await signed(member, `/v1/orgs/${orgId}/vault`),
      "OrgVaultState",
    );
    expect(seen.vault?.holding).toBeUndefined();
    expect((await remake(member, orgId)).status).toBe(403);
  });

  it("is refused once the org's Team plan has lapsed", async () => {
    const { orgId, owner, admin } = await vault({ lapsed: true });
    await lastSeen(owner, 60);

    expect((await remake(admin, orgId)).status).toBe(403);
  });

  it("is refused from a generation the vault has moved on from", async () => {
    const { orgId, owner, admin } = await vault();
    await lastSeen(owner, 60);

    expect((await remake(admin, orgId, 2)).status).toBe(409);
  });

  it("is refused while a computer that would seal the key for the asker may be online", async () => {
    const { orgId, owner, admin } = await vault();

    connected.add(owner.key);
    await lastSeen(owner, 60);
    const online = await remake(admin, orgId);
    expect(online.status).toBe(409);
    expect((await body(online, "ApiError")).error.message).toContain(
      "Studio Mac",
    );

    connected.delete(owner.key);
    await lastSeen(owner, 1);
    expect((await remake(admin, orgId)).status).toBe(409);

    await lastSeen(owner, 60);
    expect((await remake(admin, orgId)).status).toBe(200);
  });
});
