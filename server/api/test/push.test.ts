import { generateKeyPairSync } from "node:crypto";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Writable } from "node:stream";
import {
  LiveCloseCode,
  type LivePush,
  type PushTokenRegistration,
} from "@sikemux/protocol";
import { pino } from "pino";
import {
  afterAll,
  afterEach,
  beforeAll,
  beforeEach,
  describe,
  expect,
  it,
} from "vitest";

import vector from "../../protocol/vectors/push-token.json" with { type: "json" };
import type { Database } from "../src/db.ts";
import { pushTokenMessage, signedBy } from "../src/devices/signature.ts";
import { RateLimiter } from "../src/limits.ts";
import { ApnsProvider } from "../src/push/apns.ts";
import { FcmProvider, readServiceAccount } from "../src/push/fcm.ts";
import type { PushMessage } from "../src/push/provider.ts";
import { Pusher } from "../src/push/send.ts";
import { readPush } from "../src/push/settings.ts";
import {
  caller,
  challenge,
  emptyTables,
  migratedDatabase,
  newDevice,
  registered,
  signText,
  type TestDevice,
} from "./accounts.ts";
import { fakeFcm, fcmError, type FakeFcm } from "./fcm.ts";
import {
  clientPeer,
  hostPeer,
  runApi,
  type Peer,
  type Running,
} from "./live.ts";
import { body, testApp } from "./support.ts";
import { macToken, sessionToken } from "./tokens.ts";

const apnsKey = generateKeyPairSync("ec", {
  namedCurve: "prime256v1",
}).privateKey;

let database: Database;
let drop: () => Promise<void>;
let fcm: FakeFcm;
let api: Running | undefined;
let app: ReturnType<typeof testApp>;
const peers: Peer[] = [];

beforeAll(async () => {
  ({ database, drop } = await migratedDatabase());
});

beforeEach(async () => {
  await emptyTables(database);
  fcm = await fakeFcm();
  app = testApp(database);
});

afterEach(async () => {
  for (const peer of peers.splice(0)) peer.close();
  await api?.stop();
  api = undefined;
  await fcm.stop();
});

afterAll(() => drop());

const call = (path: string, token: string, init = {}) =>
  caller(app)(path, token, init);

const phoneToken = (userId: string, sessionId = "sess_phone") =>
  sessionToken(userId, { claims: { sid: sessionId } });

async function tokenRegistration(
  device: TestDevice,
  token: string,
  sessionToken: string,
  fields: Partial<PushTokenRegistration> = {},
): Promise<PushTokenRegistration> {
  const nonce = await challenge(app, sessionToken);
  return {
    platform: "fcm",
    token,
    app: "production",
    nonce,
    signature: signText(device, pushTokenMessage(nonce, device.key, token)),
    ...fields,
  };
}

async function putToken(
  userId: string,
  device: TestDevice,
  token: string,
  fields: Partial<PushTokenRegistration> = {},
) {
  const session = await phoneToken(userId);
  return call(`/v1/devices/${device.key}/push`, session, {
    method: "PUT",
    json: await tokenRegistration(device, token, session, fields),
  });
}

async function storedTokens() {
  const { rows } = await database.pool.query<{
    device_key: string;
    token: string;
    failures: number;
    last_ok_at: Date | null;
  }>(
    "select device_key, token, failures, last_ok_at from push_tokens order by token",
  );
  return rows;
}

/** A host and a phone on one account, the phone with an FCM token. */
async function pair(userId = "user_a", token = "fcm-token-a") {
  const mac = await registered(app, userId, "host");
  const phone = await registered(app, userId, "client");
  expect((await putToken(userId, phone, token)).status).toBe(200);
  return { mac, phone };
}

function push(to: string, fields: Partial<LivePush> = {}): LivePush {
  return {
    type: "push",
    ref: 1,
    to,
    kind: "alert",
    collapseId: "0123456789abcdef0123456789abcdef",
    blob: Buffer.from("sealed notification").toString("base64"),
    expiresAt: new Date(Date.now() + 120_000).toISOString(),
    ...fields,
  };
}

function kept<T extends Peer>(peer: T): T {
  peers.push(peer);
  return peer;
}

async function startLive(
  options: Parameters<typeof runApi>[2] = {},
): Promise<Running> {
  api = await runApi(
    database,
    {},
    {
      providers: {
        fcm: new FcmProvider(fcm.account, { endpoint: fcm.endpoint }),
      },
      limits: { retryDelaysMs: [0, 0, 0] },
      ...options,
    },
  );
  app = api.app;
  return api;
}

async function liveHost(device: TestDevice) {
  const peer = kept(await hostPeer(api?.url ?? "", device));
  await peer.until("ready");
  return peer;
}

async function pushed(peer: Peer, message: LivePush) {
  peer.send(message);
  const answer = await peer.until("pushed");
  expect(answer.ref).toBe(message.ref);
  return answer.result;
}

describe("push token signatures", () => {
  it("verify the vector the phone signs", () => {
    const message = pushTokenMessage(vector.nonce, vector.key, vector.token);
    expect(message).toBe(vector.message);
    expect(signedBy(vector.key, message, vector.signature)).toBe(true);
  });

  it("fail for another token", () => {
    expect(
      signedBy(
        vector.key,
        pushTokenMessage(vector.nonce, vector.key, "another token"),
        vector.signature,
      ),
    ).toBe(false);
  });
});

describe("PUT /v1/devices/{key}/push", () => {
  it("stores the phone's token, signed by its key", async () => {
    const phone = await registered(app, "user_a", "client");
    const response = await putToken("user_a", phone, "fcm-token-a");
    expect(response.status).toBe(200);
    expect(await body(response, "PushTokenState")).toMatchObject({
      enabled: true,
    });
    expect(await storedTokens()).toMatchObject([
      { device_key: phone.key, token: "fcm-token-a", failures: 0 },
    ]);
  });

  it("replaces the phone's earlier token", async () => {
    const phone = await registered(app, "user_a", "client");
    await putToken("user_a", phone, "fcm-old");
    await putToken("user_a", phone, "fcm-new");
    expect((await storedTokens()).map((row) => row.token)).toEqual(["fcm-new"]);
  });

  it("moves a token that another phone had to this one", async () => {
    const before = await registered(app, "user_a", "client", {
      sessionId: "sess_phone",
    });
    const after = await registered(app, "user_a", "client", {
      sessionId: "sess_phone",
    });
    await putToken("user_a", before, "fcm-reinstalled");
    expect((await putToken("user_a", after, "fcm-reinstalled")).status).toBe(
      200,
    );
    expect(await storedTokens()).toMatchObject([
      { device_key: after.key, token: "fcm-reinstalled" },
    ]);
  });

  it("refuses a signature that is not the phone's", async () => {
    const phone = await registered(app, "user_a", "client");
    const session = await phoneToken("user_a");
    const json = await tokenRegistration(phone, "fcm-token", session);
    json.signature = signText(
      newDevice(),
      pushTokenMessage(json.nonce, phone.key, "fcm-token"),
    );
    const response = await call(`/v1/devices/${phone.key}/push`, session, {
      method: "PUT",
      json,
    });
    expect(response.status).toBe(403);
    expect(await storedTokens()).toEqual([]);
  });

  it("refuses a challenge used twice", async () => {
    const phone = await registered(app, "user_a", "client");
    const session = await phoneToken("user_a");
    const json = await tokenRegistration(phone, "fcm-token", session);
    const first = await call(`/v1/devices/${phone.key}/push`, session, {
      method: "PUT",
      json,
    });
    expect(first.status).toBe(200);
    const again = await call(`/v1/devices/${phone.key}/push`, session, {
      method: "PUT",
      json,
    });
    expect(again.status).toBe(400);
  });

  it("does not let another account point a phone anywhere", async () => {
    const phone = await registered(app, "user_a", "client");
    const response = await putToken("user_b", phone, "fcm-token");
    expect(response.status).toBe(404);
  });

  it("refuses hosts and the Mac app's sign-in", async () => {
    const mac = await registered(app, "user_a", "host");
    expect((await putToken("user_a", mac, "fcm-token")).status).toBe(404);
    const phone = await registered(app, "user_a", "client");
    const response = await call(
      `/v1/devices/${phone.key}/push`,
      await macToken("user_a"),
      {
        method: "PUT",
        json: await tokenRegistration(
          phone,
          "fcm-token",
          await phoneToken("user_a"),
        ),
      },
    );
    expect(response.status).toBe(403);
  });

  it("refuses the other app's tokens, and Apple's sandbox unless allowed", async () => {
    const phone = await registered(app, "user_a", "client");
    expect(
      (await putToken("user_a", phone, "fcm-dev", { app: "dev" })).status,
    ).toBe(400);
    expect(
      (
        await putToken("user_a", phone, "apns-sandbox", {
          platform: "apns",
          apnsEnvironment: "sandbox",
        })
      ).status,
    ).toBe(400);
    expect(
      (await putToken("user_a", phone, "apns-no-env", { platform: "apns" }))
        .status,
    ).toBe(400);
    expect(
      (
        await putToken("user_a", phone, "apns-prod", {
          platform: "apns",
          apnsEnvironment: "production",
        })
      ).status,
    ).toBe(200);

    app = testApp(database, new RateLimiter(), {
      push: { app: "production", allowSandbox: true },
    });
    expect(
      (
        await putToken("user_a", phone, "apns-sandbox", {
          platform: "apns",
          apnsEnvironment: "sandbox",
        })
      ).status,
    ).toBe(200);
  });
});

describe("DELETE /v1/devices/{key}/push", () => {
  it("forgets the token, and answers the same when there is none", async () => {
    const phone = await registered(app, "user_a", "client");
    await putToken("user_a", phone, "fcm-token");
    const session = await phoneToken("user_a");
    for (let i = 0; i < 2; i += 1) {
      const response = await call(`/v1/devices/${phone.key}/push`, session, {
        method: "DELETE",
      });
      expect(response.status).toBe(204);
    }
    expect(await storedTokens()).toEqual([]);
  });

  it("is only for your own devices", async () => {
    const phone = await registered(app, "user_a", "client");
    await putToken("user_a", phone, "fcm-token");
    const response = await call(
      `/v1/devices/${phone.key}/push`,
      await phoneToken("user_b"),
      { method: "DELETE" },
    );
    expect(response.status).toBe(404);
    expect(await storedTokens()).toHaveLength(1);
  });

  it("refuses the Mac app's sign-in, as setting one does", async () => {
    const phone = await registered(app, "user_a", "client");
    await putToken("user_a", phone, "fcm-token");
    const response = await call(
      `/v1/devices/${phone.key}/push`,
      await macToken("user_a"),
      { method: "DELETE" },
    );
    expect(response.status).toBe(403);
    expect(await storedTokens()).toHaveLength(1);
  });
});

describe("tokens leave with their phone", () => {
  it("when the phone is removed", async () => {
    const { phone } = await pair();
    const response = await call(
      `/v1/devices/${phone.key}`,
      await sessionToken("user_a", { claims: { sid: "sess_web" } }),
      { method: "DELETE" },
    );
    expect(response.status).toBe(204);
    expect(await storedTokens()).toEqual([]);
  });

  it("when the account is deleted", async () => {
    await pair();
    const response = await call(
      "/v1/account",
      await sessionToken("user_a", {
        claims: { sid: "sess_phone", fva: [1, -1] },
      }),
      { method: "DELETE" },
    );
    expect(response.status).toBe(202);
    expect(await storedTokens()).toEqual([]);
  });
});

describe("FCM", () => {
  const message: PushMessage = {
    token: "fcm-token",
    kind: "alert",
    collapseId: "0123456789abcdef0123456789abcdef",
    blob: "c2VhbGVk",
    ttlSeconds: 119.2,
  };

  it("mints one access token and keeps it until it nearly expires", async () => {
    let now = Date.now();
    const provider = new FcmProvider(fcm.account, {
      endpoint: fcm.endpoint,
      now: () => now,
    });
    const sends = await Promise.all([
      provider.send(message),
      provider.send(message),
    ]);
    expect(sends).toEqual([{ outcome: "sent" }, { outcome: "sent" }]);
    expect(fcm.mints).toBe(1);

    now += 50 * 60_000;
    await provider.send(message);
    expect(fcm.mints).toBe(1);

    now += 6 * 60_000;
    fcm.nextAccessToken = "ya29.second";
    await provider.send(message);
    expect(fcm.mints).toBe(2);
  });

  it("sends data only, so the phone builds the notification", async () => {
    const provider = new FcmProvider(fcm.account, { endpoint: fcm.endpoint });
    await provider.send(message);
    await provider.send({ ...message, kind: "clear", ttlSeconds: -3 });
    expect(fcm.sent.map((sent) => sent.body)).toEqual([
      {
        token: "fcm-token",
        data: {
          b: "c2VhbGVk",
          c: "0123456789abcdef0123456789abcdef",
          t: "alert",
        },
        android: { priority: "high", ttl: "120s" },
      },
      {
        token: "fcm-token",
        data: {
          b: "c2VhbGVk",
          c: "0123456789abcdef0123456789abcdef",
          t: "clear",
        },
        android: { priority: "normal", ttl: "0s" },
      },
    ]);
  });

  it("calls a token dead when FCM says it is unregistered or invalid", async () => {
    const provider = new FcmProvider(fcm.account, { endpoint: fcm.endpoint });
    fcm.replies.push(
      fcmError(404, "UNREGISTERED"),
      fcmError(400, "INVALID_ARGUMENT"),
      fcmError(403, "SENDER_ID_MISMATCH"),
    );
    for (const reason of [
      "UNREGISTERED",
      "INVALID_ARGUMENT",
      "SENDER_ID_MISMATCH",
    ])
      expect(await provider.send(message)).toEqual({
        outcome: "dead",
        reason,
      });
  });

  it("asks for a retry on 429 and 5xx, with FCM's Retry-After", async () => {
    const provider = new FcmProvider(fcm.account, { endpoint: fcm.endpoint });
    fcm.replies.push(
      { ...fcmError(429, "QUOTA_EXCEEDED"), headers: { "retry-after": "7" } },
      fcmError(503, "UNAVAILABLE"),
    );
    expect(await provider.send(message)).toEqual({
      outcome: "retry",
      reason: "QUOTA_EXCEEDED",
      afterMs: 7_000,
    });
    expect(await provider.send(message)).toEqual({
      outcome: "retry",
      reason: "UNAVAILABLE",
    });
  });

  it("mints a fresh access token when FCM stops accepting the old one", async () => {
    const provider = new FcmProvider(fcm.account, { endpoint: fcm.endpoint });
    fcm.replies.push({ status: 401, body: {} });
    fcm.nextAccessToken = "ya29.second";
    expect(await provider.send(message)).toEqual({ outcome: "sent" });
    expect(fcm.mints).toBe(2);
  });

  it("fails without retrying when Google refuses the service account", async () => {
    const provider = new FcmProvider(
      { ...fcm.account, clientEmail: "someone-else@example.com" },
      { endpoint: fcm.endpoint },
    );
    expect(await provider.send(message)).toMatchObject({ outcome: "failed" });
    expect(fcm.sent).toEqual([]);
  });
});

describe("FCM settings", () => {
  let dir: string;
  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), "sikemux-fcm-"));
  });
  afterEach(() => rmSync(dir, { recursive: true, force: true }));

  it("read the service account's key file", () => {
    const path = join(dir, "fcm.json");
    writeFileSync(path, JSON.stringify(fcm.keyFile));
    const problems: string[] = [];
    const settings = readPush({ FCM_SERVICE_ACCOUNT_FILE: path }, problems);
    expect(problems).toEqual([]);
    expect(settings).toMatchObject({
      app: "production",
      allowSandbox: false,
      fcm: {
        projectId: "sikemux-test",
        clientEmail: fcm.account.clientEmail,
        privateKeyId: "key1",
        tokenUri: fcm.account.tokenUri,
      },
    });
  });

  it("are optional, so the API starts without push", () => {
    const problems: string[] = [];
    expect(readPush({}, problems)).toEqual({
      app: "production",
      allowSandbox: false,
      fcm: null,
      apns: null,
    });
    expect(readPush({ PUSH_APP: "dev" }, problems).allowSandbox).toBe(true);
    expect(problems).toEqual([]);
  });

  it("name what is wrong with a key file", () => {
    const missing = join(dir, "missing.json");
    const broken = join(dir, "broken.json");
    writeFileSync(broken, JSON.stringify({ type: "authorized_user" }));
    const problems: string[] = [];
    readPush({ FCM_SERVICE_ACCOUNT_FILE: missing, PUSH_APP: "beta" }, problems);
    readPush({ FCM_SERVICE_ACCOUNT_FILE: broken }, problems);
    expect(problems).toEqual([
      "PUSH_APP is not one of production, dev",
      `FCM_SERVICE_ACCOUNT_FILE ${missing} cannot be read (ENOENT)`,
      `FCM_SERVICE_ACCOUNT_FILE ${broken} is not a service account key with project_id, client_email and private_key`,
    ]);
    expect(() => readServiceAccount(missing)).toThrow("cannot be read");
  });
});

describe("a host's push", () => {
  it("reaches its phone through FCM", async () => {
    await startLive();
    const { mac, phone } = await pair();
    const peer = await liveHost(mac);
    const message = push(phone.key, { ref: 41 });
    expect(await pushed(peer, message)).toBe("sent");
    expect(fcm.sent).toMatchObject([
      {
        token: "fcm-token-a",
        body: { data: { b: message.blob, c: message.collapseId, t: "alert" } },
      },
    ]);
    expect((await storedTokens())[0]?.last_ok_at).not.toBeNull();
  });

  it("reaches an iPhone through the APNs server its token came from", async () => {
    const sent: { origin: string; path: unknown }[] = [];
    await startLive({
      providers: {
        apns: new ApnsProvider(
          { keyId: "ABC123DEFG", teamId: "D577WD6Z5U", privateKey: apnsKey },
          {
            topic: "com.nodelike.sikemux.mobile",
            endpoints: {
              production: "https://prod",
              sandbox: "https://sandbox",
            },
            transport: (origin, headers) => {
              sent.push({ origin, path: headers[":path"] });
              return Promise.resolve({ status: 200, headers: {}, body: "" });
            },
          },
        ),
      },
    });
    const mac = await registered(app, "user_a", "host");
    const phone = await registered(app, "user_a", "client");
    expect(
      (
        await putToken("user_a", phone, "apns-token", {
          platform: "apns",
          apnsEnvironment: "production",
        })
      ).status,
    ).toBe(200);
    const peer = await liveHost(mac);
    expect(await pushed(peer, push(phone.key))).toBe("sent");
    expect(sent).toEqual([
      { origin: "https://prod", path: "/3/device/apns-token" },
    ]);
  });

  it("goes only to phones on the host's own account", async () => {
    await startLive();
    const { mac } = await pair("user_a", "fcm-token-a");
    const other = await pair("user_b", "fcm-token-b");
    const peer = await liveHost(mac);
    expect(await pushed(peer, push(other.phone.key))).toBe("not_allowed");
    expect(await pushed(peer, push(other.mac.key))).toBe("not_allowed");
    expect(await pushed(peer, push(mac.key))).toBe("not_allowed");
    expect(await pushed(peer, push(newDevice().key))).toBe("not_allowed");
    expect(fcm.sent).toEqual([]);
  });

  it("does not reach a removed phone", async () => {
    await startLive();
    const { mac, phone } = await pair();
    await call(
      `/v1/devices/${phone.key}`,
      await sessionToken("user_a", { claims: { sid: "sess_web" } }),
      { method: "DELETE" },
    );
    const peer = await liveHost(mac);
    expect(await pushed(peer, push(phone.key))).toBe("not_allowed");
  });

  it("answers no_token for a phone with notifications off", async () => {
    await startLive();
    const mac = await registered(app, "user_a", "host");
    const phone = await registered(app, "user_a", "client");
    const peer = await liveHost(mac);
    expect(await pushed(peer, push(phone.key))).toBe("no_token");
  });

  it("is refused from a phone", async () => {
    await startLive();
    const { phone } = await pair();
    const peer = kept(await clientPeer(api?.url ?? "", phone, "user_a"));
    await peer.until("ready");
    peer.send(push(phone.key));
    expect(await peer.closed).toBe(LiveCloseCode.badMessage);
  });

  it("forgets a token FCM says is unregistered", async () => {
    await startLive();
    const { mac, phone } = await pair();
    const peer = await liveHost(mac);
    fcm.replies.push(fcmError(404, "UNREGISTERED"));
    expect(await pushed(peer, push(phone.key))).toBe("no_token");
    expect(await storedTokens()).toEqual([]);
  });

  it("retries a busy FCM three times, then fails", async () => {
    await startLive();
    const { mac, phone } = await pair();
    const peer = await liveHost(mac);
    for (let i = 0; i < 4; i += 1)
      fcm.replies.push(fcmError(503, "UNAVAILABLE"));
    expect(await pushed(peer, push(phone.key))).toBe("failed");
    expect(fcm.sent).toHaveLength(4);
    expect(await storedTokens()).toMatchObject([{ failures: 1 }]);

    fcm.replies.push(fcmError(429, "QUOTA_EXCEEDED"));
    expect(await pushed(peer, push(phone.key, { ref: 2 }))).toBe("sent");
    expect(await storedTokens()).toMatchObject([{ failures: 0 }]);
  });

  it("is throttled past the limits for one host and one phone", async () => {
    await startLive({
      limits: { perPair: 2, perPhone: 3, retryDelaysMs: [] },
    });
    const { mac, phone } = await pair();
    const second = await registered(app, "user_a", "host");
    const peer = await liveHost(mac);
    const results = [];
    for (let ref = 0; ref < 3; ref += 1)
      results.push(await pushed(peer, push(phone.key, { ref })));
    expect(results).toEqual(["sent", "sent", "throttled"]);

    const other = await liveHost(second);
    expect(await pushed(other, push(phone.key, { ref: 7 }))).toBe("sent");
    expect(await pushed(other, push(phone.key, { ref: 8 }))).toBe("throttled");
  });

  it("is dropped once it has expired", async () => {
    await startLive();
    const { mac, phone } = await pair();
    const peer = await liveHost(mac);
    const stale = push(phone.key, {
      expiresAt: new Date(Date.now() - 1_000).toISOString(),
    });
    expect(await pushed(peer, stale)).toBe("expired");
    expect(fcm.sent).toEqual([]);
  });

  it("answers not_set_up when this server cannot reach the platform", async () => {
    await startLive({ providers: {} });
    const { mac, phone } = await pair();
    const peer = await liveHost(mac);
    expect(await pushed(peer, push(phone.key))).toBe("not_set_up");
  });
});

describe("the push log", () => {
  it("names devices by key prefix and never holds the blob", async () => {
    const lines: string[] = [];
    const capture = pino(
      { level: "debug" },
      new Writable({
        write(chunk: Buffer, _encoding, done) {
          lines.push(chunk.toString());
          done();
        },
      }),
    );
    const { mac, phone } = await pair();
    const pusher = new Pusher({
      db: database.db,
      log: capture,
      limiter: new RateLimiter(),
      providers: {
        fcm: new FcmProvider(fcm.account, { endpoint: fcm.endpoint }),
      },
    });
    const message = push(phone.key, {
      blob: Buffer.from("a very secret prompt").toString("base64"),
    });
    expect(await pusher.push({ userId: "user_a", key: mac.key }, message)).toBe(
      "sent",
    );
    const text = lines.join("");
    expect(text).toContain('"result":"sent"');
    expect(text).toContain(phone.key.slice(0, 8));
    expect(text).not.toContain(message.blob);
    expect(text).not.toContain(phone.key);
    expect(text).not.toContain("fcm-token-a");
  });
});
