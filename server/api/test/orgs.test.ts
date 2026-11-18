import type { OrgRole } from "@sikemux/protocol";
import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";

import type { Database } from "../src/db.ts";
import { caller, emptyTables, events, migratedDatabase } from "./accounts.ts";
import { teamPlan } from "./billing.ts";
import { FakeClerk } from "./clerk.ts";
import { body, testApp } from "./support.ts";
import { sessionToken } from "./tokens.ts";

let database: Database;
let drop: () => Promise<void>;
let app: ReturnType<typeof testApp>;
let call: ReturnType<typeof caller>;

const OWNER = "user_owner";
const ADMIN = "user_admin";
const MEMBER = "user_member";
const NEWCOMER = "user_newcomer";

beforeAll(async () => {
  ({ database, drop } = await migratedDatabase());
});

beforeEach(async () => {
  await emptyTables(database);
  await database.pool.query("truncate orgs cascade");
  await database.pool.query(
    "insert into users (id, staff) select unnest($1::text[]), true",
    [[OWNER, ADMIN, MEMBER, NEWCOMER]],
  );
  app = testApp(database, undefined, { clerk: new FakeClerk() });
  call = caller(app);
});

afterAll(() => drop());

async function as(
  userId: string,
  path: string,
  init: { method?: string; json?: unknown } = {},
) {
  return call(path, await sessionToken(userId), init);
}

async function createOrg(seats: number) {
  const response = await as(OWNER, "/v1/orgs", {
    method: "POST",
    json: { name: "Acme" },
  });
  expect(response.status).toBe(201);
  const org = await body(response, "Org");
  if (seats > 0) await teamPlan(database.db, org.id, seats);
  return org.id;
}

async function invite(by: string, orgId: string, role: OrgRole) {
  return as(by, `/v1/orgs/${orgId}/invites`, {
    method: "POST",
    json: { role },
  });
}

async function join(userId: string, token: string) {
  return as(userId, "/v1/orgs/join", { method: "POST", json: { token } });
}

async function addMember(orgId: string, userId: string, role: OrgRole) {
  const response = await invite(OWNER, orgId, role);
  expect(response.status).toBe(201);
  const { token } = await body(response, "OrgInviteCreated");
  expect((await join(userId, token)).status).toBe(200);
}

/** An org with an owner, an admin and a user, and seats to spare. */
async function fullOrg() {
  const orgId = await createOrg(10);
  await addMember(orgId, ADMIN, "admin");
  await addMember(orgId, MEMBER, "user");
  return orgId;
}

async function memberId(orgId: string, userId: string) {
  const { id } = await database.db
    .selectFrom("org_users")
    .select("id")
    .where("org_id", "=", orgId)
    .where("user_id", "=", userId)
    .executeTakeFirstOrThrow();
  return id;
}

async function setRole(
  by: string,
  orgId: string,
  userId: string,
  role: OrgRole,
) {
  return as(
    by,
    `/v1/orgs/${orgId}/members/${await memberId(orgId, userId)}/role`,
    { method: "PUT", json: { role } },
  );
}

async function remove(by: string, orgId: string, userId: string) {
  return as(by, `/v1/orgs/${orgId}/members/${await memberId(orgId, userId)}`, {
    method: "DELETE",
  });
}

describe("orgs", () => {
  it("answer only staff accounts", async () => {
    await database.pool.query("update users set staff = false where id = $1", [
      NEWCOMER,
    ]);
    expect((await as(NEWCOMER, "/v1/orgs")).status).toBe(404);
    expect(
      (
        await as(NEWCOMER, "/v1/orgs", {
          method: "POST",
          json: { name: "Acme" },
        })
      ).status,
    ).toBe(404);
  });

  it("let a user only look", async () => {
    const orgId = await fullOrg();
    expect((await as(MEMBER, `/v1/orgs/${orgId}/members`)).status).toBe(200);
    expect(
      (
        await as(MEMBER, `/v1/orgs/${orgId}/name`, {
          method: "POST",
          json: { name: "Mine" },
        })
      ).status,
    ).toBe(403);
    expect((await invite(MEMBER, orgId, "user")).status).toBe(403);
    expect((await as(MEMBER, `/v1/orgs/${orgId}/invites`)).status).toBe(403);
    expect((await setRole(MEMBER, orgId, MEMBER, "admin")).status).toBe(403);
    expect((await remove(MEMBER, orgId, ADMIN)).status).toBe(403);
    expect(
      (await as(MEMBER, `/v1/orgs/${orgId}`, { method: "DELETE" })).status,
    ).toBe(403);
  });

  it("let an admin manage admins and users but not owners", async () => {
    const orgId = await fullOrg();
    expect(
      (
        await as(ADMIN, `/v1/orgs/${orgId}/name`, {
          method: "POST",
          json: { name: "Acme Two" },
        })
      ).status,
    ).toBe(200);
    expect((await invite(ADMIN, orgId, "admin")).status).toBe(201);
    expect((await invite(ADMIN, orgId, "owner")).status).toBe(403);
    expect((await setRole(ADMIN, orgId, MEMBER, "owner")).status).toBe(403);
    expect((await setRole(ADMIN, orgId, OWNER, "user")).status).toBe(403);
    expect((await remove(ADMIN, orgId, OWNER)).status).toBe(403);
    expect((await setRole(ADMIN, orgId, MEMBER, "admin")).status).toBe(200);
    expect((await remove(ADMIN, orgId, MEMBER)).status).toBe(204);
    expect(
      (await as(ADMIN, `/v1/orgs/${orgId}`, { method: "DELETE" })).status,
    ).toBe(403);
  });

  it("keep the last owner", async () => {
    const orgId = await fullOrg();
    expect(
      (await as(OWNER, `/v1/orgs/${orgId}/leave`, { method: "POST" })).status,
    ).toBe(409);
    expect((await setRole(OWNER, orgId, OWNER, "admin")).status).toBe(409);
    expect((await remove(OWNER, orgId, OWNER)).status).toBe(409);

    expect((await setRole(OWNER, orgId, ADMIN, "owner")).status).toBe(200);
    expect(
      (await as(OWNER, `/v1/orgs/${orgId}/leave`, { method: "POST" })).status,
    ).toBe(204);
    expect((await setRole(ADMIN, orgId, ADMIN, "user")).status).toBe(409);
  });

  it("tell a person's apps their plan changed when their seat starts or ends", async () => {
    const orgId = await createOrg(10);
    const planChanges = async () =>
      (await events(database))
        .filter((event) => event.type === "plan.changed")
        .map((event) => event.user_id);

    await addMember(orgId, ADMIN, "admin");
    await addMember(orgId, MEMBER, "user");
    expect(await planChanges()).toEqual([ADMIN, MEMBER]);

    expect((await setRole(OWNER, orgId, MEMBER, "admin")).status).toBe(200);
    expect(await planChanges()).toEqual([ADMIN, MEMBER]);

    expect((await remove(OWNER, orgId, MEMBER)).status).toBe(204);
    expect(
      (await as(ADMIN, `/v1/orgs/${orgId}/leave`, { method: "POST" })).status,
    ).toBe(204);
    expect(await planChanges()).toEqual([ADMIN, MEMBER, MEMBER, ADMIN]);
  });

  it("refuse deleting the account of an org's only owner while others are in it", async () => {
    await fullOrg();
    const token = await sessionToken(OWNER, { claims: { fva: [0, -1] } });
    const response = await call("/v1/account", token, { method: "DELETE" });
    expect(response.status).toBe(409);
  });
});

describe("invites", () => {
  it("work once", async () => {
    const orgId = await createOrg(3);
    const { token } = await body(
      await invite(OWNER, orgId, "user"),
      "OrgInviteCreated",
    );
    const joined = await join(MEMBER, token);
    expect(joined.status).toBe(200);
    expect((await body(joined, "Org")).role).toBe("user");
    expect((await join(NEWCOMER, token)).status).toBe(404);
  });

  it("expire", async () => {
    const orgId = await createOrg(3);
    const { token } = await body(
      await invite(OWNER, orgId, "user"),
      "OrgInviteCreated",
    );
    await database.pool.query(
      "update org_invites set expires_at = now() - interval '1 second'",
    );
    expect((await join(MEMBER, token)).status).toBe(404);
  });

  it("name their org to a reader with no session only while they would join", async () => {
    const preview = async (token: string) => {
      const response = await app.request("/v1/orgs/join/preview", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ token }),
      });
      expect(response.status).toBe(200);
      return (await body(response, "OrgInvitePreview")).orgName;
    };
    const orgId = await createOrg(5);
    const created = async () =>
      (await body(await invite(OWNER, orgId, "user"), "OrgInviteCreated"))
        .token;
    const live = await created();
    expect(await preview(live)).toBe("Acme");

    const used = await created();
    expect((await join(MEMBER, used)).status).toBe(200);
    const expired = await created();
    await database.pool.query(
      "update org_invites set expires_at = now() - interval '1 second' where token_hash = encode(sha256($1::bytea), 'hex')",
      [expired],
    );
    const revoked = await created();
    await database.pool.query(
      "update org_invites set revoked_at = now() where token_hash = encode(sha256($1::bytea), 'hex')",
      [revoked],
    );
    for (const token of [used, expired, revoked, "A".repeat(43)])
      expect(await preview(token)).toBeNull();
    expect(await preview(live)).toBe("Acme");
  });

  it("stop at the seat limit, with each pending invite holding a seat", async () => {
    const orgId = await createOrg(3);
    expect((await invite(OWNER, orgId, "user")).status).toBe(201);
    const second = await body(
      await invite(OWNER, orgId, "user"),
      "OrgInviteCreated",
    );
    expect((await invite(OWNER, orgId, "user")).status).toBe(409);

    const revoked = await as(
      OWNER,
      `/v1/orgs/${orgId}/invites/${second.invite.id}`,
      { method: "DELETE" },
    );
    expect(revoked.status).toBe(204);
    expect((await join(MEMBER, second.token)).status).toBe(404);
    expect((await invite(OWNER, orgId, "user")).status).toBe(201);
  });

  it("are refused while the org has no Team plan", async () => {
    const orgId = await createOrg(0);
    expect((await invite(OWNER, orgId, "user")).status).toBe(409);
  });
});
