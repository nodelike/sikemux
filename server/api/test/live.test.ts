import { LiveCloseCode } from "@sikemux/protocol";
import {
  afterAll,
  afterEach,
  beforeAll,
  beforeEach,
  describe,
  expect,
  it,
  vi,
} from "vitest";

import type { Database } from "../src/db.ts";
import { registrationMessage } from "../src/devices/signature.ts";
import { LIVE_OPTIONS } from "../src/live/options.ts";
import { LiveSocket } from "../src/live/socket.ts";
import {
  caller,
  challenge,
  emptyTables,
  migratedDatabase,
  newDevice,
  registered,
  registration,
  signLive,
  signText,
  type TestDevice,
} from "./accounts.ts";
import {
  clientPeer,
  hostPeer,
  Peer,
  runApi,
  upgradeStatus,
  webPeer,
  type Running,
} from "./live.ts";
import { APP_ORIGIN } from "./support.ts";
import { macToken, sessionToken } from "./tokens.ts";

let database: Database;
let drop: () => Promise<void>;
let api: Running;
const peers: Peer[] = [];

beforeAll(async () => {
  ({ database, drop } = await migratedDatabase());
});

beforeEach(async () => {
  await emptyTables(database);
});

afterEach(async () => {
  for (const peer of peers.splice(0)) peer.close();
  await api?.stop();
  vi.restoreAllMocks();
});

afterAll(() => drop());

async function start(options: Parameters<typeof runApi>[1] = {}) {
  api = await runApi(database, options);
  return api;
}

function kept<T extends Peer>(peer: T): T {
  peers.push(peer);
  return peer;
}

async function removeAs(userId: string, key: string, sessionId = "sess_web") {
  const response = await caller(api.app)(
    `/v1/devices/${key}`,
    await sessionToken(userId, { claims: { sid: sessionId } }),
    { method: "DELETE" },
  );
  expect(response.status).toBe(204);
}

async function host(userId: string, device?: TestDevice) {
  return registered(api.app, userId, "host", device ? { device } : {});
}

async function phone(userId: string, sessionId = "sess_phone") {
  return registered(api.app, userId, "client", { sessionId });
}

describe("a host's live connection", () => {
  it("opens with a challenge, then ready, then hears a phone removed", async () => {
    await start();
    const mac = await host("user_a");
    const iphone = await phone("user_a");
    const peer = kept(await hostPeer(api.url, mac));
    const ready = await peer.until("ready");
    expect(ready.heartbeatMs).toBe(LIVE_OPTIONS.heartbeatMs);
    expect(await peer.quiet()).toEqual([]);

    await removeAs("user_a", iphone.key);
    const frame = await peer.until("events");
    expect(frame.events).toMatchObject([
      {
        type: "device.revoked",
        key: iphone.key,
        role: "client",
        reason: "removed",
      },
    ]);
  });

  it("catches up from its cursor after being offline, and not again once acknowledged", async () => {
    await start();
    const mac = await host("user_a");
    const first = await phone("user_a", "sess_1");
    const second = await phone("user_a", "sess_2");
    await removeAs("user_a", first.key);
    await removeAs("user_a", second.key);

    const peer = await hostPeer(api.url, mac);
    await peer.until("ready");
    const frame = await peer.until("events");
    expect(frame.events.map((event) => event.key)).toEqual([
      first.key,
      second.key,
    ]);
    peer.send({ type: "ack", id: frame.events[1]?.id ?? 0 });
    await new Promise((resolve) => setTimeout(resolve, 100));
    peer.close();
    await peer.closed;

    const again = kept(await hostPeer(api.url, mac));
    await again.until("ready");
    expect(await again.quiet()).toEqual([]);
  });

  it("sends at most two frames ahead of the last acknowledgement", async () => {
    await start({ eventsPerFrame: 1 });
    const mac = await host("user_a");
    for (const sid of ["s1", "s2", "s3", "s4"])
      await removeAs("user_a", (await phone("user_a", sid)).key);

    const peer = kept(await hostPeer(api.url, mac));
    await peer.until("ready");
    const frames = await peer.quiet(300);
    expect(frames.map((frame) => frame.type)).toEqual(["events", "events"]);
    const firstId = frames[0]?.type === "events" ? frames[0].events[0]?.id : 0;
    peer.send({ type: "ack", id: firstId ?? 0 });
    expect((await peer.quiet(300)).map((frame) => frame.type)).toEqual([
      "events",
    ]);
  });

  it("is told to reset when events after its cursor were pruned", async () => {
    await start();
    const mac = await host("user_a");
    await database.pool.query(
      "update users set events_pruned_through = 1000000 where id = 'user_a'",
    );
    const peer = kept(await hostPeer(api.url, mac));
    await peer.until("ready");
    expect((await peer.next()).type).toBe("reset");
  });

  it("is replaced by a newer connection for the same device", async () => {
    await start();
    const mac = await host("user_a");
    const older = await hostPeer(api.url, mac);
    await older.until("ready");
    const newer = kept(await hostPeer(api.url, mac));
    await newer.until("ready");
    expect(await older.closed).toBe(LiveCloseCode.replaced);
  });

  it("answers pings and is pinged", async () => {
    await start({ heartbeatMs: 50 });
    const mac = await host("user_a");
    const peer = kept(await hostPeer(api.url, mac));
    await peer.until("ready");
    await peer.until("ping");
    peer.send({ type: "pong" });
  });

  it("says bye with a reconnect hint when the API stops", async () => {
    await start({ reconnectSpreadMs: 3_000 });
    const mac = await host("user_a");
    const peer = await hostPeer(api.url, mac);
    await peer.until("ready");
    await api.live.stop();
    const bye = await peer.until("bye");
    expect(bye.reconnectAfterMs).toBeGreaterThanOrEqual(1_000);
    expect(bye.reconnectAfterMs).toBeLessThanOrEqual(3_000);
    expect(await peer.closed).toBe(LiveCloseCode.restarting);
  });

  it("says bye even to connections that have not said hello yet", async () => {
    await start();
    const peer = new Peer(api.url);
    await peer.challenge();
    await api.live.stop();
    expect((await peer.until("bye")).type).toBe("bye");
    expect(await peer.closed).toBe(LiveCloseCode.restarting);
  });

  it("records when the device was last seen", async () => {
    await start();
    const mac = await host("user_a");
    const peer = await hostPeer(api.url, mac);
    await peer.until("ready");
    const { rows } = await database.pool.query(
      "select last_seen_at is not null as seen from devices where key = $1",
      [mac.key],
    );
    expect(rows).toEqual([{ seen: true }]);
    peer.close();
  });
});

describe("a removed device", () => {
  it("gets its backlog, then revoked and 4403 while connected, never ready again", async () => {
    await start();
    const mac = await host("user_a");
    const iphone = await phone("user_a");
    const peer = await clientPeer(api.url, iphone, "user_a");
    await peer.until("ready");
    await removeAs("user_a", iphone.key);
    const revoked = await peer.until("revoked");
    expect(revoked.reason).toBe("removed");
    expect(await peer.closed).toBe(LiveCloseCode.revoked);
    expect(mac.key).toBeDefined();
  });

  it("as a host coming back: backlog, revoked, 4403; after purge, 4401", async () => {
    await start();
    const mac = await host("user_a");
    const iphone = await phone("user_a");
    await removeAs("user_a", iphone.key);
    await removeAs("user_a", mac.key);

    const peer = await hostPeer(api.url, mac);
    const seen: string[] = [];
    for (;;) {
      const message = await peer.next();
      seen.push(message.type);
      if (message.type === "events")
        expect(message.events.map((event) => event.key)).toEqual([
          iphone.key,
          mac.key,
        ]);
      if (message.type === "revoked") {
        expect(message.reason).toBe("removed");
        break;
      }
    }
    expect(seen).toEqual(["events", "revoked"]);
    expect(await peer.closed).toBe(LiveCloseCode.revoked);

    await database.pool.query("delete from removed_devices");
    const purged = await hostPeer(api.url, mac);
    expect(await purged.closed).toBe(LiveCloseCode.unauthenticated);
    expect(purged.messages.map((message) => message.type)).toEqual([
      "challenge",
    ]);
  });

  it("leaves with leave: only itself, as signed out, and the hosts hear it", async () => {
    await start();
    const mac = await host("user_a");
    const other = await host("user_a");
    const iphone = await phone("user_a");
    const hostSide = kept(await hostPeer(api.url, mac));
    await hostSide.until("ready");

    const peer = await clientPeer(api.url, iphone, "user_a");
    await peer.until("ready");
    peer.send({ type: "leave" });
    expect((await peer.until("revoked")).reason).toBe("signed_out");
    expect(await peer.closed).toBe(LiveCloseCode.revoked);

    const frame = await hostSide.until("events");
    expect(frame.events).toMatchObject([
      { type: "device.revoked", key: iphone.key, reason: "signed_out" },
    ]);
    const { rows } = await database.pool.query(
      "select key from devices order by key",
    );
    expect(rows.map((row) => row.key).sort()).toEqual(
      [mac.key, other.key].sort(),
    );
    const audit = await database.pool.query(
      "select actor from audit where action = 'device.removed'",
    );
    expect(audit.rows).toEqual([{ actor: `device:${iphone.key}` }]);
  });

  it("cannot name another device to leave", async () => {
    await start();
    const mac = await host("user_a");
    const other = await host("user_a");
    const peer = await hostPeer(api.url, mac);
    await peer.until("ready");
    peer.send({ type: "leave", key: other.key });
    expect(await peer.closed).toBe(LiveCloseCode.badMessage);
    const { rows } = await database.pool.query(
      "select count(*)::int as n from devices",
    );
    expect(rows[0].n).toBe(2);
  });
});

describe("events never cross accounts", () => {
  it("reach only their own account's connections", async () => {
    await start();
    const macA = await host("user_a");
    const macB = await host("user_b");
    const phoneA = await phone("user_a", "sess_a");
    const phoneB = await phone("user_b", "sess_b");
    const a = kept(await hostPeer(api.url, macA));
    const b = kept(await hostPeer(api.url, macB));
    const pa = kept(
      await clientPeer(api.url, phoneA, "user_a", { sessionId: "sess_a" }),
    );
    const pb = kept(
      await clientPeer(api.url, phoneB, "user_b", { sessionId: "sess_b" }),
    );
    for (const peer of [a, b, pa, pb]) await peer.until("ready");

    const wake = vi.spyOn(LiveSocket.prototype, "wake");
    await host("user_a");
    await removeAs("user_a", phoneA.key, "sess_web");
    await new Promise((resolve) => setTimeout(resolve, 200));
    const woken = new Set(wake.mock.calls.map(([who]) => who.userId));
    expect(woken).toEqual(new Set(["user_a"]));

    const fromB = [...(await b.quiet()), ...(await pb.quiet())];
    expect(fromB).toEqual([]);
    const fromA = await a.quiet();
    expect(
      fromA.flatMap((m) => (m.type === "events" ? m.events : [])),
    ).toMatchObject([{ type: "device.revoked", key: phoneA.key }]);
  });

  it("and phones never hear of other phones", async () => {
    await start();
    const mac = await host("user_a");
    const mine = await phone("user_a", "sess_1");
    const peer = kept(
      await clientPeer(api.url, mine, "user_a", { sessionId: "sess_1" }),
    );
    await peer.until("ready");
    const other = await phone("user_a", "sess_2");
    await removeAs("user_a", other.key);
    await removeAs("user_a", mac.key);
    const frame = await peer.until("events");
    expect(frame.events.map((event) => event.key)).toEqual([mac.key]);
    expect(await peer.quiet()).toEqual([]);
  });
});

describe("replay", () => {
  it("refuses a hello replayed on a new connection", async () => {
    await start();
    const mac = await host("user_a");
    const first = await hostPeer(api.url, mac);
    await first.until("ready");
    const nonce = (first.messages[0] as { nonce: string }).nonce;
    first.close();

    const second = new Peer(api.url);
    await second.challenge();
    second.send({
      type: "hello",
      role: "host",
      key: mac.key,
      signature: signLive(mac, nonce),
    });
    expect(await second.closed).toBe(LiveCloseCode.unauthenticated);
  });

  it("closes a second hello on the same connection", async () => {
    await start();
    const mac = await host("user_a");
    const peer = await hostPeer(api.url, mac);
    await peer.until("ready");
    peer.send({
      type: "hello",
      role: "host",
      key: mac.key,
      signature: "0".repeat(128),
    });
    expect(await peer.closed).toBe(LiveCloseCode.badMessage);
  });

  it("refuses a hello after the challenge expired", async () => {
    await start({ challengeMs: 50 });
    const mac = await host("user_a");
    const peer = new Peer(api.url);
    const nonce = await peer.challenge();
    await new Promise((resolve) => setTimeout(resolve, 100));
    peer.send({
      type: "hello",
      role: "host",
      key: mac.key,
      signature: signLive(mac, nonce),
    });
    expect(await peer.closed).toBe(LiveCloseCode.unauthenticated);
  });

  it("never takes a registration signature as a live one, or the reverse", async () => {
    await start();
    const mac = await host("user_a");
    const peer = new Peer(api.url);
    const nonce = await peer.challenge();
    peer.send({
      type: "hello",
      role: "host",
      key: mac.key,
      signature: signText(mac, registrationMessage(nonce, "user_a", mac.key)),
    });
    expect(await peer.closed).toBe(LiveCloseCode.unauthenticated);

    const token = await macToken("user_a");
    const registerNonce = await challenge(api.app, token);
    const response = await caller(api.app)("/v1/devices", token, {
      method: "POST",
      json: await registration(api.app, mac, "user_a", token, {
        nonce: registerNonce,
        signature: signLive(mac, registerNonce),
      }),
    });
    expect(response.status).toBe(403);
  });
});

describe("impersonation", () => {
  it("refuses a host signing for another host's key", async () => {
    await start();
    const victim = await host("user_a");
    const attacker = newDevice();
    const peer = new Peer(api.url);
    const nonce = await peer.challenge();
    peer.send({
      type: "hello",
      role: "host",
      key: victim.key,
      signature: signLive({ ...attacker, key: victim.key }, nonce),
    });
    expect(await peer.closed).toBe(LiveCloseCode.unauthenticated);
  });

  it("refuses one account's token with another account's phone", async () => {
    await start();
    const theirs = await phone("user_b", "sess_b");
    const peer = await clientPeer(api.url, theirs, "user_a", {
      sessionId: "sess_a",
    });
    expect(await peer.closed).toBe(LiveCloseCode.unauthenticated);
  });

  it("refuses a host key as a client, and a client key as a host", async () => {
    await start();
    const mac = await host("user_a");
    const iphone = await phone("user_a");
    const asClient = await clientPeer(api.url, mac, "user_a");
    expect(await asClient.closed).toBe(LiveCloseCode.unauthenticated);
    const asHost = await hostPeer(api.url, iphone);
    expect(await asHost.closed).toBe(LiveCloseCode.unauthenticated);
  });

  it("refuses a client without its session, or with the Mac's token", async () => {
    await start();
    const iphone = await phone("user_a");
    for (const token of [undefined, await macToken("user_a")]) {
      const peer = new Peer(api.url);
      const nonce = await peer.challenge();
      peer.send({
        type: "hello",
        role: "client",
        key: iphone.key,
        signature: signLive(iphone, nonce),
        ...(token ? { token } : {}),
      });
      expect(await peer.closed).toBe(LiveCloseCode.unauthenticated);
    }
  });

  it("refuses the Mac's OAuth token, or a phone's session, as the web app", async () => {
    await start();
    const mac = await webPeer(api.url, "user_a", await macToken("user_a"));
    expect(await mac.closed).toBe(LiveCloseCode.unauthenticated);
    const native = await webPeer(
      api.url,
      "user_a",
      await sessionToken("user_a"),
    );
    expect(await native.closed).toBe(LiveCloseCode.unauthenticated);
  });

  it("refuses a deleted account's web token", async () => {
    await start();
    await host("user_a");
    await database.pool.query("update users set deleted_at = now()");
    const peer = await webPeer(api.url, "user_a");
    expect(await peer.closed).toBe(LiveCloseCode.unauthenticated);
  });

  it("refuses browsers on other sites", async () => {
    await start();
    expect(
      await upgradeStatus(api.url, { origin: "https://evil.example" }),
    ).toBe(403);
    expect(await upgradeStatus(api.url, { origin: APP_ORIGIN })).toBe(101);
  });

  it("lets in the phone, whose WebSocket names the API as its origin", async () => {
    await start();
    const { origin } = new URL(api.url.replace(/^ws/, "http"));
    expect(await upgradeStatus(api.url, { origin })).toBe(101);
  });
});

describe("the web app's connection", () => {
  it("starts at the latest event and hears everything on the account", async () => {
    await start();
    const mac = await host("user_a");
    const peer = kept(await webPeer(api.url, "user_a"));
    const ready = await peer.until("ready");
    expect(ready.latest).toBeGreaterThan(0);
    expect(ready.authExpiresAt).toBeDefined();
    expect(await peer.quiet()).toEqual([]);
    const iphone = await phone("user_a");
    const frame = await peer.until("events");
    expect(frame.events).toMatchObject([
      { type: "device.added", key: iphone.key },
    ]);
    expect(mac.key).toBeDefined();
  });

  it("closes when its token expires, unless it sends a fresh one", async () => {
    await start({ authGraceMs: 100 });
    const stale = kept(
      await webPeer(
        api.url,
        "user_a",
        await sessionToken("user_a", {
          expiresIn: "1s",
          claims: { azp: APP_ORIGIN },
        }),
      ),
    );
    const fresh = kept(
      await webPeer(
        api.url,
        "user_a",
        await sessionToken("user_a", {
          expiresIn: "1s",
          claims: { azp: APP_ORIGIN },
        }),
      ),
    );
    await stale.until("ready");
    await fresh.until("ready");
    fresh.send({
      type: "auth",
      token: await sessionToken("user_a", { claims: { azp: APP_ORIGIN } }),
    });
    expect(await stale.closed).toBe(LiveCloseCode.unauthenticated);
    expect(fresh.closeCode).toBeUndefined();
  });

  it("refuses a fresh token for another account", async () => {
    await start();
    const peer = await webPeer(api.url, "user_a");
    await peer.until("ready");
    peer.send({
      type: "auth",
      token: await sessionToken("user_b", { claims: { azp: APP_ORIGIN } }),
    });
    expect(await peer.closed).toBe(LiveCloseCode.unauthenticated);
  });

  it("is not a device: it cannot leave", async () => {
    await start();
    const peer = await webPeer(api.url, "user_a");
    await peer.until("ready");
    peer.send({ type: "leave" });
    expect(await peer.closed).toBe(LiveCloseCode.badMessage);
  });
});

describe("limits", () => {
  it("refuses the 61st upgrade from one address in a minute", async () => {
    await start();
    const headers = { "x-forwarded-for": "203.0.113.1" };
    const statuses: number[] = [];
    for (let i = 0; i < 61; i++)
      statuses.push(await upgradeStatus(api.url, headers));
    expect(statuses.slice(0, 60).every((status) => status === 101)).toBe(true);
    expect(statuses[60]).toBe(429);
    expect(
      await upgradeStatus(api.url, { "x-forwarded-for": "203.0.113.2" }),
    ).toBe(101);
  });

  it("refuses an address after ten failed hellos", async () => {
    await start();
    const headers = { "x-forwarded-for": "203.0.113.3" };
    for (let i = 0; i < 10; i++) {
      const peer = new Peer(api.url, headers);
      await peer.challenge();
      peer.send({
        type: "hello",
        role: "host",
        key: newDevice().key,
        signature: "0".repeat(128),
      });
      expect(await peer.closed).toBe(LiveCloseCode.unauthenticated);
    }
    expect(await upgradeStatus(api.url, headers)).toBe(429);
  });

  it("refuses a key after ten failed hellos, from anywhere", async () => {
    await start();
    const mac = await host("user_a");
    for (let i = 0; i < 10; i++) {
      const peer = new Peer(api.url, { "x-forwarded-for": `198.51.100.${i}` });
      await peer.challenge();
      peer.send({
        type: "hello",
        role: "host",
        key: mac.key,
        signature: "0".repeat(128),
      });
      await peer.closed;
    }
    const peer = await hostPeer(api.url, mac, {
      "x-forwarded-for": "198.51.100.200",
    });
    expect(await peer.closed).toBe(LiveCloseCode.overLimit);
  });

  it("refuses the 21st connection for one account", async () => {
    await start();
    const open: Peer[] = [];
    for (let i = 0; i < 20; i++) {
      const peer = kept(await webPeer(api.url, "user_a"));
      await peer.until("ready");
      open.push(peer);
    }
    const extra = await webPeer(api.url, "user_a");
    expect(await extra.closed).toBe(LiveCloseCode.overLimit);
    expect(open.every((peer) => peer.closeCode === undefined)).toBe(true);
  });

  it("closes a connection sending more than 20 messages a second", async () => {
    await start();
    const mac = await host("user_a");
    const peer = await hostPeer(api.url, mac);
    await peer.until("ready");
    for (let i = 0; i < 21; i++) peer.send({ type: "pong" });
    expect(await peer.closed).toBe(LiveCloseCode.overLimit);
  });

  it("closes a connection sending a 9 KB frame", async () => {
    await start();
    const peer = new Peer(api.url);
    await peer.challenge();
    peer.raw("x".repeat(9 * 1024));
    expect(await peer.closed).toBe(1009);
  });

  it("closes a connection that never says hello, by default after 10 seconds", async () => {
    expect(LIVE_OPTIONS.helloTimeoutMs).toBe(10_000);
    await start({ helloTimeoutMs: 100 });
    const peer = new Peer(api.url);
    await peer.challenge();
    expect(await peer.closed).toBe(LiveCloseCode.helloTimeout);
  });

  it("closes a message that is not part of the protocol", async () => {
    await start();
    const peer = new Peer(api.url);
    await peer.challenge();
    peer.send({ type: "subscribe", all: true });
    expect(await peer.closed).toBe(LiveCloseCode.badMessage);
  });

  it("answers 404 for any other WebSocket path", async () => {
    await start();
    expect(
      await upgradeStatus(api.url.replace("/v1/live", "/v1/devices")),
    ).toBe(404);
  });
});
