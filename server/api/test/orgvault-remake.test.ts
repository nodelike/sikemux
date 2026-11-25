import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";

import type { Database } from "../src/db.ts";
import { Presence } from "../src/live/hub.ts";
import {
  emptyTables,
  hostAuthorization,
  migratedDatabase,
  registered,
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

function signed(
  host: TestDevice,
  path: string,
  method = "GET",
  json?: unknown,
) {
  const text = json === undefined ? "" : JSON.stringify(json);
  return app.request(path, {
    method,
    headers: {
      authorization: hostAuthorization(host, method, path, text),
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
    (
      await signed(owner, `/v1/orgs/${id}/vault`, "POST", {
        key: "owner",
        approvedOnly: true,
      })
    ).status,
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
    approvedOnly: true,
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

  it("is refused while the API cannot yet tell who is online", async () => {
    const { orgId, owner, admin } = await vault();
    await lastSeen(owner, 60);

    app = testApp(database, undefined, {
      clerk: new FakeClerk(),
      presence: new Presence(),
    });
    expect((await remake(admin, orgId)).status).toBe(409);

    const started = new Presence();
    started.attach({ online: () => false });
    app = testApp(database, undefined, {
      clerk: new FakeClerk(),
      presence: started,
    });
    expect((await remake(admin, orgId)).status).toBe(409);
  });
});

async function entry(orgId: string, id: string, kind: string) {
  await database.db
    .insertInto("org_vault_entries")
    .values({
      id,
      org_id: orgId,
      kind,
      generation: 1,
      sealed: "sealed",
      revision: 3,
      updated_by: "host",
    })
    .execute();
}

describe("replacing an org's vault key", () => {
  const STORE = "a".repeat(32);
  const WAITING = "b".repeat(32);
  const MODEL_KEY = "c".repeat(32);

  async function filled() {
    const made = await vault();
    await entry(made.orgId, STORE, "project-memory");
    await entry(made.orgId, WAITING, "project-memory-waiting");
    await entry(made.orgId, MODEL_KEY, "model-key");
    const rotate = (kept: string[], dropped: string[]) =>
      signed(made.owner, `/v1/orgs/${made.orgId}/vault/rotate`, "POST", {
        from: 1,
        keys: [{ hostKey: made.owner.key, wrapped: "next" }],
        entries: kept.map((id) => ({ id, sealed: "next", revision: 3 })),
        drops: dropped.map((id) => ({ id, revision: 3 })),
        approvedOnly: true,
      });
    return { ...made, rotate };
  }

  it("removes the project entries it names, whose ids no later entry takes back", async () => {
    const { orgId, owner, rotate } = await filled();
    expect((await rotate([STORE, MODEL_KEY], [WAITING])).status).toBe(200);

    const seen = await body(
      await signed(owner, `/v1/orgs/${orgId}/vault`),
      "OrgVaultState",
    );
    expect(seen.vault?.generation).toBe(2);
    expect(seen.vault?.entries.map(({ id }) => id).sort()).toEqual([
      STORE,
      MODEL_KEY,
    ]);
    expect(seen.vault?.ended).toEqual([{ id: WAITING, revision: 4 }]);
  });

  it("takes no write while a host that left still holds the old one", async () => {
    const { orgId, owner, rotate } = await filled();
    await database.db
      .insertInto("org_vault_keys")
      .values({
        org_id: orgId,
        host_key: "f".repeat(64),
        generation: 1,
        wrapped: "left",
        wrapped_by: owner.key,
      })
      .execute();
    const write = (generation: number, revision: number) =>
      signed(owner, `/v1/orgs/${orgId}/vault/entries/${MODEL_KEY}`, "PUT", {
        kind: "model-key",
        generation,
        sealed: "new",
        revision,
      });
    expect((await write(1, 3)).status).toBe(503);
    expect((await rotate([STORE, WAITING, MODEL_KEY], [])).status).toBe(200);
    expect((await write(2, 4)).status).toBe(200);
  });

  it("removes nothing an owner or admin wrote", async () => {
    const { rotate } = await filled();
    expect((await rotate([STORE, WAITING], [MODEL_KEY])).status).toBe(403);
  });

  it("names every entry once, kept or removed", async () => {
    const { rotate } = await filled();
    expect((await rotate([STORE, MODEL_KEY], [])).status).toBe(409);
    expect((await rotate([STORE, MODEL_KEY, WAITING], [WAITING])).status).toBe(
      409,
    );
  });
});

describe("a host that does not say it seals for approved hosts alone", () => {
  it("makes, seals, replaces and makes again no org's key", async () => {
    const { orgId, owner, admin } = await vault();
    const path = `/v1/orgs/${orgId}/vault`;
    const keys = [{ hostKey: owner.key, wrapped: "next" }];
    const asked: [TestDevice, string, string, unknown][] = [
      [admin, path, "POST", { key: "key" }],
      [owner, `${path}/keys`, "PUT", { generation: 1, keys }],
      [owner, `${path}/rotate`, "POST", { from: 1, keys, entries: [] }],
      [owner, `${path}/remake`, "POST", { from: 1, key: "key" }],
    ];
    for (const [host, at, method, json] of asked) {
      const refused = await signed(host, at, method, json);
      expect(refused.status).toBe(403);
      expect((await body(refused, "ApiError")).error.message).toContain(
        "too old",
      );
    }
  });
});

describe("a member's word for the host that let theirs in", () => {
  it("is taken only while that host's membership lasts", async () => {
    const { orgId, admin, member } = await vault();
    const members = await database.db
      .selectFrom("org_users")
      .select(["id", "user_id"])
      .where("org_id", "=", orgId)
      .execute();
    const of = (user: string) =>
      members.find((row) => row.user_id === user)?.id ?? "";
    const signature = "A".repeat(86);
    await database.db
      .insertInto("org_vault_vouches")
      .values({
        org_id: orgId,
        host_key: member.key,
        member: of(MEMBER),
        by_key: admin.key,
        by_member: of(ADMIN),
        at: 1,
        signature,
        introduction: "introduction",
      })
      .execute();
    await database.db
      .insertInto("org_vault_keys")
      .values({
        org_id: orgId,
        host_key: member.key,
        generation: 1,
        wrapped: "member",
        wrapped_by: admin.key,
      })
      .execute();
    const vouched = async () => {
      const put = await signed(
        member,
        `/v1/orgs/${orgId}/vault/vouches`,
        "PUT",
        { vouches: [{ hostKey: admin.key, at: 2, signature }] },
      );
      expect(put.status).toBe(200);
      const { numDeletedRows } = await database.db
        .deleteFrom("org_vault_vouches")
        .where("by_key", "=", member.key)
        .executeTakeFirstOrThrow();
      return Number(numDeletedRows);
    };
    expect(await vouched()).toBe(1);

    await database.db
      .deleteFrom("org_users")
      .where("id", "=", of(ADMIN))
      .execute();
    await database.db
      .insertInto("org_users")
      .values({ org_id: orgId, user_id: ADMIN, role: "admin" })
      .execute();
    expect(await vouched()).toBe(0);
  });
});
