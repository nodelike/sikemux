import pg from "pg";
import {
  afterAll,
  beforeAll,
  beforeEach,
  describe,
  expect,
  it,
  vi,
} from "vitest";

import type { Database } from "../src/db.ts";
import { backlog } from "../src/events/backlog.ts";
import { appendEvent, latestEventId } from "../src/events/log.ts";
import {
  caller,
  emptyTables,
  events,
  migratedDatabase,
  newDevice,
  registered,
  registration,
} from "./accounts.ts";
import { testApp } from "./support.ts";
import { macToken, sessionToken } from "./tokens.ts";

let database: Database;
let drop: () => Promise<void>;
let app: ReturnType<typeof testApp>;
let call: ReturnType<typeof caller>;

beforeAll(async () => {
  ({ database, drop } = await migratedDatabase());
});

beforeEach(async () => {
  await emptyTables(database);
  app = testApp(database);
  call = caller(app);
});

afterAll(() => drop());

async function cursorOf(key: string) {
  const { rows } = await database.pool.query<{ acked: number }>(
    "select acked_event_id::int as acked from devices where key = $1",
    [key],
  );
  return rows[0]?.acked;
}

async function tombstone(key: string) {
  const { rows } = await database.pool.query(
    "select user_id, role, reason, acked_event_id::int as acked, clerk_session_id from removed_devices where key = $1",
    [key],
  );
  return rows[0];
}

describe("the event log", () => {
  it("records a device added, and changed when it registers again", async () => {
    const mac = await registered(app, "user_a", "host");
    await registered(app, "user_a", "host", { device: mac });
    expect(await events(database)).toMatchObject([
      { user_id: "user_a", type: "device.added", subject: mac.key },
      { user_id: "user_a", type: "device.changed", subject: mac.key },
    ]);
  });

  it("starts a new device at the end of its account's log", async () => {
    await registered(app, "user_a", "host");
    await registered(app, "user_a", "host");
    const phone = await registered(app, "user_a", "client");
    expect(await cursorOf(phone.key)).toBe(
      await latestEventId(database.db, "user_a"),
    );
  });

  it("notifies listeners with the account, and only once the change commits", async () => {
    const listener = new pg.Client({
      connectionString: database.pool.options.connectionString,
    });
    await listener.connect();
    const heard: string[] = [];
    listener.on("notification", (note) => heard.push(note.payload ?? ""));
    await listener.query("listen sikemux_events");

    await database.db
      .transaction()
      .execute(async (trx) => {
        await trx.insertInto("users").values({ id: "user_a" }).execute();
        await appendEvent(trx, { userId: "user_a", type: "account.deleted" });
        throw new Error("roll back");
      })
      .catch(() => undefined);
    await registered(app, "user_b", "host");
    await vi.waitFor(() => expect(heard).not.toEqual([]));
    await listener.end();

    expect(heard).toEqual(["user_b"]);
    expect(await events(database)).toHaveLength(1);
  });
});

describe("removing a device", () => {
  it("moves it to a tombstone and tells the account in one go", async () => {
    const mac = await registered(app, "user_a", "host");
    const phone = await registered(app, "user_a", "client", {
      sessionId: "sess_phone",
    });
    const response = await call(
      `/v1/devices/${phone.key}`,
      await sessionToken("user_a", { claims: { sid: "sess_web" } }),
      { method: "DELETE" },
    );
    expect(response.status).toBe(204);

    const log = await events(database);
    expect(log.at(-1)).toMatchObject({
      type: "device.revoked",
      subject: phone.key,
      subject_role: "client",
      reason: "removed",
    });
    expect(await tombstone(phone.key)).toEqual({
      user_id: "user_a",
      role: "client",
      reason: "removed",
      acked: log.at(-2)?.id,
      clerk_session_id: "sess_phone",
    });
    const { rows } = await database.pool.query(
      "select actor, detail from audit where action = 'device.removed'",
    );
    expect(rows).toEqual([
      {
        actor: "user:user_a",
        detail: expect.objectContaining({ role: "client", reason: "removed" }),
      },
    ]);
    expect(await cursorOf(mac.key)).toBeDefined();
  });

  it("is a sign-out when the phone removes itself with its own session", async () => {
    const phone = await registered(app, "user_a", "client", {
      sessionId: "sess_phone",
    });
    await call(
      `/v1/devices/${phone.key}`,
      await sessionToken("user_a", { claims: { sid: "sess_phone" } }),
      { method: "DELETE" },
    );
    expect((await tombstone(phone.key))?.reason).toBe("signed_out");
  });

  it("writes nothing for a device that is not yours", async () => {
    const mac = await registered(app, "user_b", "host");
    const before = await events(database);
    const response = await call(
      `/v1/devices/${mac.key}`,
      await sessionToken("user_a", { claims: { sid: "sess_web" } }),
      { method: "DELETE" },
    );
    expect(response.status).toBe(404);
    expect(await events(database)).toEqual(before);
    expect(await tombstone(mac.key)).toBeUndefined();
  });
});

describe("a removed client registering again", () => {
  it("is refused with the session that was removed", async () => {
    const phone = await registered(app, "user_a", "client", {
      sessionId: "sess_stolen",
    });
    await call(
      `/v1/devices/${phone.key}`,
      await sessionToken("user_a", { claims: { sid: "sess_web" } }),
      { method: "DELETE" },
    );
    const stolen = await sessionToken("user_a", {
      claims: { sid: "sess_stolen" },
    });
    for (const [path, method] of [
      ["/v1/devices/challenge", "POST"],
      ["/v1/devices", "POST"],
      ["/v1/devices", "GET"],
    ] as const) {
      const json = method === "POST" ? {} : undefined;
      expect((await call(path, stolen, { method, json })).status).toBe(401);
    }
  });

  it("may sign in again with a new session, and starts at the latest event, not its old cursor", async () => {
    const phone = await registered(app, "user_a", "client", {
      sessionId: "sess_old",
    });
    await call(
      `/v1/devices/${phone.key}`,
      await sessionToken("user_a", { claims: { sid: "sess_web" } }),
      { method: "DELETE" },
    );
    await registered(app, "user_a", "host");
    await registered(app, "user_a", "client", {
      device: phone,
      sessionId: "sess_new",
    });
    expect(await cursorOf(phone.key)).toBe(
      await latestEventId(database.db, "user_a"),
    );
    expect(await tombstone(phone.key)).toBeUndefined();
  });
});

describe("a deleted account", () => {
  async function markDeleted(userId: string) {
    await database.pool.query(
      "update users set deleted_at = now() where id = $1",
      [userId],
    );
  }

  it("has its unexpired tokens refused everywhere", async () => {
    await registered(app, "user_a", "host");
    await markDeleted("user_a");
    const session = await sessionToken("user_a");
    expect((await call("/v1/devices", session)).status).toBe(401);
    expect(
      (await call("/v1/devices/challenge", session, { method: "POST" })).status,
    ).toBe(401);
    expect((await call("/v1/devices", await macToken("user_a"))).status).toBe(
      401,
    );
    expect(
      (await call("/v1/devices", await sessionToken("user_b"))).status,
    ).toBe(200);
  });

  it("is not brought back by registering", async () => {
    await registered(app, "user_a", "host");
    await markDeleted("user_a");
    const token = await macToken("user_a");
    expect(
      (await call("/v1/devices/challenge", token, { method: "POST" })).status,
    ).toBe(401);
    expect(
      (
        await call("/v1/devices", token, {
          method: "POST",
          json: await registration(app, newDevice(), "user_a", token, {
            nonce: "a".repeat(64),
          }),
        })
      ).status,
    ).toBe(401);
    const { rows } = await database.pool.query(
      "select deleted_at is not null as deleted from users where id = 'user_a'",
    );
    expect(rows).toEqual([{ deleted: true }]);
  });
});

describe("what each device may read from the log", () => {
  async function account(userId: string) {
    const host = await registered(app, userId, "host");
    const otherHost = await registered(app, userId, "host");
    const phone = await registered(app, userId, "client", {
      sessionId: `sess_${userId}_1`,
    });
    const otherPhone = await registered(app, userId, "client", {
      sessionId: `sess_${userId}_2`,
    });
    await call(`/v1/devices/${otherPhone.key}`, await sessionToken(userId), {
      method: "DELETE",
    });
    await call(`/v1/devices/${otherHost.key}`, await sessionToken(userId), {
      method: "DELETE",
    });
    return { host, otherHost, phone, otherPhone };
  }

  function seen(list: Awaited<ReturnType<typeof backlog>>) {
    return list.map((event) => `${event.type} ${event.key}`);
  }

  it("shows a host the clients revoked, itself and nothing else of phones", async () => {
    const a = await account("user_a");
    const visible = await backlog(database.db, {
      userId: "user_a",
      role: "host",
      key: a.host.key,
      after: 0,
      limit: 200,
    });
    expect(seen(visible)).toEqual([
      `device.added ${a.host.key}`,
      `device.revoked ${a.otherPhone.key}`,
    ]);
  });

  it("never shows a phone another phone", async () => {
    const a = await account("user_a");
    const visible = await backlog(database.db, {
      userId: "user_a",
      role: "client",
      key: a.phone.key,
      after: 0,
      limit: 200,
    });
    expect(seen(visible)).toEqual([
      `device.added ${a.host.key}`,
      `device.added ${a.otherHost.key}`,
      `device.added ${a.phone.key}`,
      `device.revoked ${a.otherHost.key}`,
    ]);
  });

  it("shows the web app everything on the account", async () => {
    await account("user_a");
    const visible = await backlog(database.db, {
      userId: "user_a",
      role: "web",
      key: null,
      after: 0,
      limit: 200,
    });
    expect(visible).toHaveLength(6);
    expect(visible.every((event) => event.id > 0)).toBe(true);
  });

  it("never crosses accounts, whatever key and cursor it is given", async () => {
    const a = await account("user_a");
    const b = await account("user_b");
    for (const role of ["host", "client", "web"] as const) {
      const visible = await backlog(database.db, {
        userId: "user_b",
        role,
        key: role === "client" ? a.phone.key : a.host.key,
        after: 0,
        limit: 200,
      });
      const aKeys = new Set(Object.values(a).map((device) => device.key));
      expect(visible.some((event) => aKeys.has(event.key ?? ""))).toBe(false);
    }
    const fromB = await backlog(database.db, {
      userId: "user_b",
      role: "host",
      key: b.host.key,
      after: 0,
      limit: 200,
    });
    expect(fromB.length).toBeGreaterThan(0);
  });

  it("pages by cursor and limit", async () => {
    await account("user_a");
    const first = await backlog(database.db, {
      userId: "user_a",
      role: "web",
      key: null,
      after: 0,
      limit: 2,
    });
    const cursor = first.at(-1)?.id ?? 0;
    expect(first).toHaveLength(2);
    const rest = await backlog(database.db, {
      userId: "user_a",
      role: "web",
      key: null,
      after: cursor,
      limit: 200,
    });
    expect(rest).toHaveLength(4);
    expect(rest.every((event) => event.id > cursor)).toBe(true);
  });
});
