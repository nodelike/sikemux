import type { DeviceRegistration } from "@sikemux/protocol";
import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";

import type { Database } from "../src/db.ts";
import { registrationMessage } from "../src/devices/signature.ts";
import { RateLimiter } from "../src/limits.ts";
import * as harness from "./accounts.ts";
import {
  caller,
  migratedDatabase,
  newDevice,
  type TestDevice,
} from "./accounts.ts";
import { body, testApp } from "./support.ts";
import { macToken, sessionToken } from "./tokens.ts";

let database: Database;
let drop: () => Promise<void>;
let app: ReturnType<typeof testApp>;

beforeAll(async () => {
  ({ database, drop } = await migratedDatabase());
});

beforeEach(async () => {
  await harness.emptyTables(database);
  app = testApp(database);
});

afterAll(() => drop());

const call: ReturnType<typeof caller> = (...args) => caller(app)(...args);
const challenge = (token: string) => harness.challenge(app, token);

function registration(
  device: TestDevice,
  userId: string,
  token: string,
  fields: Partial<DeviceRegistration> = {},
) {
  return harness.registration(app, device, userId, token, {
    channel: "stable",
    ...fields,
  });
}

describe("registering a device", () => {
  it("adds a Mac to the account it proves its key for", async () => {
    const token = await macToken("user_a");
    const mac = newDevice();
    const response = await call("/v1/devices", token, {
      method: "POST",
      json: await registration(mac, "user_a", token),
    });
    expect(response.status).toBe(201);
    expect(await body(response, "Device")).toMatchObject({
      key: mac.key,
      role: "host",
      name: "Studio Mac",
      platform: "macos",
      channel: "stable",
      lastSeenAt: null,
    });
    const { rows } = await database.pool.query(
      "select action, subject, detail from audit",
    );
    expect(rows).toEqual([
      {
        action: "device.registered",
        subject: mac.key,
        detail: {
          role: "host",
          name: "Studio Mac",
          platform: "macos",
          via: "mac",
        },
      },
    ]);
  });

  it("adds a phone, which has no channel", async () => {
    const token = await sessionToken("user_a");
    const phone = newDevice();
    const json = await registration(phone, "user_a", token, {
      role: "client",
      platform: "ios",
      name: "iPhone",
    });
    delete json.channel;
    const response = await call("/v1/devices", token, { method: "POST", json });
    expect(response.status).toBe(201);
    expect(await body(response, "Device")).not.toHaveProperty("channel");
  });

  it("updates a device registered again, keeping one row and its name", async () => {
    const token = await macToken("user_a");
    const mac = newDevice();
    await call("/v1/devices", token, {
      method: "POST",
      json: await registration(mac, "user_a", token),
    });
    const again = await call("/v1/devices", token, {
      method: "POST",
      json: await registration(mac, "user_a", token, {
        name: "Renamed Mac",
        channel: "nightly",
      }),
    });
    expect(again.status).toBe(200);
    expect(await body(again, "Device")).toMatchObject({
      name: "Studio Mac",
      channel: "nightly",
    });
    expect(
      (await database.pool.query("select count(*)::int as n from devices"))
        .rows[0].n,
    ).toBe(1);
  });

  it("refuses a key another account registered", async () => {
    const mac = newDevice();
    const first = await macToken("user_a");
    await call("/v1/devices", first, {
      method: "POST",
      json: await registration(mac, "user_a", first),
    });
    const second = await macToken("user_b");
    const response = await call("/v1/devices", second, {
      method: "POST",
      json: await registration(mac, "user_b", second),
    });
    expect(response.status).toBe(409);
    expect((await body(response, "ApiError")).error.code).toBe("conflict");
  });

  it("refuses a signature made for another account", async () => {
    const token = await macToken("user_a");
    const mac = newDevice();
    const nonce = await challenge(token);
    const signature = harness.signText(
      mac,
      registrationMessage(nonce, "user_b", mac.key),
    );
    const response = await call("/v1/devices", token, {
      method: "POST",
      json: await registration(mac, "user_a", token, { nonce, signature }),
    });
    expect(response.status).toBe(403);
  });

  it("refuses a key the request does not hold", async () => {
    const token = await macToken("user_a");
    const claimed = newDevice();
    const holder = newDevice();
    const json = await registration(holder, "user_a", token, {});
    const response = await call("/v1/devices", token, {
      method: "POST",
      json: { ...json, key: claimed.key },
    });
    expect(response.status).toBe(403);
  });

  it("uses each challenge once", async () => {
    const token = await macToken("user_a");
    const mac = newDevice();
    const json = await registration(mac, "user_a", token);
    expect(
      (await call("/v1/devices", token, { method: "POST", json })).status,
    ).toBe(201);
    const replay = await call("/v1/devices", token, { method: "POST", json });
    expect(replay.status).toBe(400);
    expect((await body(replay, "ApiError")).error.message).toContain(
      "already used",
    );
  });

  it("refuses another account's challenge", async () => {
    const nonce = await challenge(await macToken("user_b"));
    const token = await macToken("user_a");
    const response = await call("/v1/devices", token, {
      method: "POST",
      json: await registration(newDevice(), "user_a", token, { nonce }),
    });
    expect(response.status).toBe(400);
  });

  it("refuses an expired challenge", async () => {
    const token = await macToken("user_a");
    const nonce = await challenge(token);
    await database.pool.query(
      "update challenges set expires_at = now() - interval '1 second'",
    );
    const response = await call("/v1/devices", token, {
      method: "POST",
      json: await registration(newDevice(), "user_a", token, { nonce }),
    });
    expect(response.status).toBe(400);
  });

  it("requires a channel from hosts and refuses one from clients", async () => {
    const token = await sessionToken("user_a");
    const host = await registration(newDevice(), "user_a", token);
    delete host.channel;
    expect(
      (await call("/v1/devices", token, { method: "POST", json: host })).status,
    ).toBe(400);
    const client = await registration(newDevice(), "user_a", token, {
      role: "client",
      platform: "android",
    });
    expect(
      (await call("/v1/devices", token, { method: "POST", json: client }))
        .status,
    ).toBe(400);
  });

  it("checks the body against the protocol", async () => {
    const token = await sessionToken("user_a");
    const response = await call("/v1/devices", token, {
      method: "POST",
      json: { key: "nope" },
    });
    expect(response.status).toBe(400);
    expect((await body(response, "ApiError")).error.message).toContain(
      "DeviceRegistration",
    );
  });

  it("needs a signed-in user", async () => {
    const response = await app.request("/v1/devices", { method: "POST" });
    expect(response.status).toBe(401);
  });
});

describe("listing devices", () => {
  it("shows only your own, newest first, filtered by role", async () => {
    const mine = await macToken("user_a");
    const mac = newDevice();
    const phone = newDevice();
    await call("/v1/devices", mine, {
      method: "POST",
      json: await registration(mac, "user_a", mine),
    });
    const phoneJson = await registration(phone, "user_a", mine, {
      role: "client",
      platform: "ios",
      name: "iPhone",
    });
    delete phoneJson.channel;
    await call("/v1/devices", mine, { method: "POST", json: phoneJson });
    const theirs = await macToken("user_b");
    await call("/v1/devices", theirs, {
      method: "POST",
      json: await registration(newDevice(), "user_b", theirs),
    });

    const all = await body(
      await call("/v1/devices", await sessionToken("user_a")),
      "DeviceList",
    );
    expect(all.devices.map((device) => device.key)).toEqual([
      phone.key,
      mac.key,
    ]);
    const hosts = await body(
      await call("/v1/devices?role=host", await sessionToken("user_a")),
      "DeviceList",
    );
    expect(hosts.devices.map((device) => device.key)).toEqual([mac.key]);
  });

  it("refuses an unknown role", async () => {
    const response = await call(
      "/v1/devices?role=admin",
      await sessionToken("user_a"),
    );
    expect(response.status).toBe(400);
  });
});

describe("removing a device", () => {
  it("takes one of your devices off the account and records it", async () => {
    const token = await macToken("user_a");
    const mac = newDevice();
    await call("/v1/devices", token, {
      method: "POST",
      json: await registration(mac, "user_a", token),
    });
    const response = await call(
      `/v1/devices/${mac.key}`,
      await sessionToken("user_a", { claims: { sid: "sess_web" } }),
      { method: "DELETE" },
    );
    expect(response.status).toBe(204);
    const list = await body(
      await call("/v1/devices", await sessionToken("user_a")),
      "DeviceList",
    );
    expect(list.devices).toEqual([]);
    const { rows } = await database.pool.query(
      "select action from audit order by id",
    );
    expect(rows.map((row) => row.action)).toEqual([
      "device.registered",
      "device.removed",
    ]);
  });

  it("cannot remove someone else's device", async () => {
    const theirs = await macToken("user_b");
    const mac = newDevice();
    await call("/v1/devices", theirs, {
      method: "POST",
      json: await registration(mac, "user_b", theirs),
    });
    const response = await call(
      `/v1/devices/${mac.key}`,
      await sessionToken("user_a", { claims: { sid: "sess_web" } }),
      { method: "DELETE" },
    );
    expect(response.status).toBe(404);
    const list = await body(
      await call("/v1/devices", await sessionToken("user_b")),
      "DeviceList",
    );
    expect(list.devices).toHaveLength(1);
  });
});

describe("rate limits", () => {
  it("caps challenges per user", async () => {
    app = testApp(database, new RateLimiter());
    const token = await macToken("user_a");
    const statuses = [];
    for (let i = 0; i < 31; i++)
      statuses.push(
        (await call("/v1/devices/challenge", token, { method: "POST" })).status,
      );
    expect(statuses.slice(0, 30).every((status) => status === 200)).toBe(true);
    expect(statuses[30]).toBe(429);
    const other = await call(
      "/v1/devices/challenge",
      await macToken("user_b"),
      { method: "POST" },
    );
    expect(other.status).toBe(200);
  });
});
