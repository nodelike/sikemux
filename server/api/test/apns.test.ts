import { generateKeyPairSync, verify, type KeyObject } from "node:crypto";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import {
  createServer,
  type Http2Server,
  type IncomingHttpHeaders,
} from "node:http2";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import type { AppleKey } from "../src/apple-key.ts";
import {
  ApnsProvider,
  GENERIC_ALERT,
  type ApnsReply,
} from "../src/push/apns.ts";
import type { PushMessage } from "../src/push/provider.ts";
import { readPush } from "../src/push/settings.ts";

interface Sent {
  headers: IncomingHttpHeaders;
  body: Record<string, unknown>;
  claims: { iss: string; iat: number };
}

interface FakeApns {
  endpoint: string;
  sent: Sent[];
  /** Answers for the next sends, used up in order; after them every send succeeds. */
  replies: {
    status: number;
    reason?: string;
    headers?: Record<string, string>;
  }[];
  stop(): Promise<void>;
}

/** APNs over cleartext HTTP/2, checking each provider token's ES256 signature. */
async function fakeApns(publicKey: KeyObject): Promise<FakeApns> {
  const server: Http2Server = createServer();
  const fake: FakeApns = {
    endpoint: "",
    sent: [],
    replies: [],
    stop: () => new Promise((resolve) => server.close(() => resolve())),
  };
  server.on("stream", (stream, headers) => {
    const chunks: Buffer[] = [];
    stream.on("data", (chunk: Buffer) => chunks.push(chunk));
    stream.on("end", () => {
      const reply = (status: number, body?: object, more = {}) => {
        stream.respond({ ":status": status, ...more });
        stream.end(body ? JSON.stringify(body) : undefined);
      };
      const token = /^bearer (\S+)$/.exec(
        String(headers.authorization ?? ""),
      )?.[1];
      const [header, claims, signature] = (token ?? "").split(".");
      if (
        !header ||
        !claims ||
        !signature ||
        !verify(
          "sha256",
          Buffer.from(`${header}.${claims}`),
          { key: publicKey, dsaEncoding: "ieee-p1363" },
          Buffer.from(signature, "base64url"),
        )
      )
        return reply(403, { reason: "InvalidProviderToken" });
      fake.sent.push({
        headers,
        body: JSON.parse(Buffer.concat(chunks).toString("utf8")) as Record<
          string,
          unknown
        >,
        claims: JSON.parse(Buffer.from(claims, "base64url").toString()) as {
          iss: string;
          iat: number;
        },
      });
      const next = fake.replies.shift();
      if (next)
        return reply(
          next.status,
          next.reason ? { reason: next.reason } : undefined,
          next.headers,
        );
      reply(200);
    });
  });
  await new Promise<void>((resolve) =>
    server.listen(0, "127.0.0.1", () => resolve()),
  );
  fake.endpoint = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  return fake;
}

const { publicKey, privateKey } = generateKeyPairSync("ec", {
  namedCurve: "prime256v1",
});
const key: AppleKey = { keyId: "ABC123DEFG", teamId: "D577WD6Z5U", privateKey };
const TOPIC = "com.nodelike.sikemux.mobile";

const message: PushMessage = {
  token: "a1b2c3",
  kind: "alert",
  collapseId: "0123456789abcdef0123456789abcdef",
  blob: "c2VhbGVk",
  ttlSeconds: 119.2,
  apnsEnvironment: "production",
};

let apns: FakeApns;
let providers: ApnsProvider[];

beforeEach(async () => {
  apns = await fakeApns(publicKey);
  providers = [];
});

afterEach(async () => {
  for (const provider of providers) provider.close();
  await apns.stop();
});

function provider(now?: () => number): ApnsProvider {
  const made = new ApnsProvider(key, {
    topic: TOPIC,
    endpoints: { production: apns.endpoint, sandbox: apns.endpoint },
    ...(now ? { now } : {}),
  });
  providers.push(made);
  return made;
}

describe("APNs", () => {
  it("sends an alert the notification extension can open and rewrite", async () => {
    const now = 1_791_000_000_000;
    expect(await provider(() => now).send(message)).toEqual({
      outcome: "sent",
    });
    const [sent] = apns.sent;
    expect(sent?.headers).toMatchObject({
      ":method": "POST",
      ":path": "/3/device/a1b2c3",
      "apns-topic": TOPIC,
      "apns-push-type": "alert",
      "apns-priority": "10",
      "apns-collapse-id": message.collapseId,
      "apns-expiration": String(1_791_000_000 + 120),
    });
    expect(sent?.body).toEqual({
      aps: { alert: GENERIC_ALERT, sound: "default", "mutable-content": 1 },
      b: "c2VhbGVk",
      c: message.collapseId,
      t: "alert",
    });
    expect(sent?.claims).toEqual({ iss: "D577WD6Z5U", iat: 1_791_000_000 });
  });

  it("sends a clear as a background push that wakes the app", async () => {
    await provider().send({ ...message, kind: "clear" });
    const [sent] = apns.sent;
    expect(sent?.headers).toMatchObject({
      "apns-push-type": "background",
      "apns-priority": "5",
    });
    expect(sent?.headers["apns-collapse-id"]).toBeUndefined();
    expect(sent?.body).toEqual({
      aps: { "content-available": 1 },
      b: "c2VhbGVk",
      c: message.collapseId,
      t: "clear",
    });
  });

  it("goes to the push server that issued the token", async () => {
    const origins: string[] = [];
    const recording = new ApnsProvider(key, {
      topic: TOPIC,
      endpoints: { production: "https://prod", sandbox: "https://sandbox" },
      transport: (origin): Promise<ApnsReply> => {
        origins.push(origin);
        return Promise.resolve({ status: 200, headers: {}, body: "" });
      },
    });
    await recording.send(message);
    await recording.send({ ...message, apnsEnvironment: "sandbox" });
    expect(origins).toEqual(["https://prod", "https://sandbox"]);
  });

  it("signs one provider token and replaces it before Apple stops taking it", async () => {
    let now = 1_791_000_000_000;
    const sender = provider(() => now);
    await sender.send(message);
    now += 49 * 60_000;
    await sender.send(message);
    now += 2 * 60_000;
    await sender.send(message);
    expect(apns.sent.map((sent) => sent.claims.iat)).toEqual([
      1_791_000_000,
      1_791_000_000,
      1_791_000_000 + 51 * 60,
    ]);
  });

  it("signs a new provider token when Apple says the old one expired", async () => {
    let now = 1_791_000_000_000;
    const sender = provider(() => now);
    await sender.send(message);
    now += 60_000;
    apns.replies.push({ status: 403, reason: "ExpiredProviderToken" });
    expect(await sender.send(message)).toEqual({ outcome: "sent" });
    expect(apns.sent.map((sent) => sent.claims.iat)).toEqual([
      1_791_000_000, 1_791_000_000, 1_791_000_060,
    ]);
  });

  it("calls a token dead when Apple will never deliver to it", async () => {
    apns.replies.push(
      { status: 410, reason: "Unregistered" },
      { status: 400, reason: "BadDeviceToken" },
      { status: 400, reason: "DeviceTokenNotForTopic" },
    );
    const sender = provider();
    for (const reason of [
      "Unregistered",
      "BadDeviceToken",
      "DeviceTokenNotForTopic",
    ])
      expect(await sender.send(message)).toEqual({ outcome: "dead", reason });
  });

  it("asks for a retry on 429 and 5xx, with Retry-After", async () => {
    apns.replies.push(
      {
        status: 429,
        reason: "TooManyRequests",
        headers: { "retry-after": "3" },
      },
      { status: 503, reason: "ServiceUnavailable" },
    );
    const sender = provider();
    expect(await sender.send(message)).toEqual({
      outcome: "retry",
      reason: "TooManyRequests",
      afterMs: 3_000,
    });
    expect(await sender.send(message)).toEqual({
      outcome: "retry",
      reason: "ServiceUnavailable",
    });
  });

  it("fails anything else, and retries when APNs cannot be reached", async () => {
    apns.replies.push({ status: 400, reason: "PayloadTooLarge" });
    expect(await provider().send(message)).toEqual({
      outcome: "failed",
      reason: "400 PayloadTooLarge",
    });
    const unreachable = new ApnsProvider(key, {
      topic: TOPIC,
      endpoints: { production: "http://127.0.0.1:1", sandbox: "" },
    });
    providers.push(unreachable);
    expect(await unreachable.send(message)).toMatchObject({
      outcome: "retry",
    });
  });
});

describe("APNs settings", () => {
  let dir: string;
  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), "sikemux-apns-"));
  });
  afterEach(() => rmSync(dir, { recursive: true, force: true }));

  const p8 = () => {
    const path = join(dir, "AuthKey_ABC123DEFG.p8");
    writeFileSync(
      path,
      privateKey.export({ format: "pem", type: "pkcs8" }).toString(),
    );
    return path;
  };

  it("read Apple's key with its ids, and take the topic from the app served", () => {
    const problems: string[] = [];
    const env = {
      APNS_KEY_FILE: p8(),
      APNS_KEY_ID: "ABC123DEFG",
      APNS_TEAM_ID: "D577WD6Z5U",
    };
    expect(readPush(env, problems).apns).toMatchObject({
      keyId: "ABC123DEFG",
      teamId: "D577WD6Z5U",
      topic: "com.nodelike.sikemux.mobile",
    });
    expect(readPush({ ...env, PUSH_APP: "dev" }, problems).apns?.topic).toBe(
      "com.nodelike.sikemux.mobile.dev",
    );
    expect(problems).toEqual([]);
  });

  it("name what is wrong", () => {
    const rsa = join(dir, "rsa.p8");
    writeFileSync(
      rsa,
      generateKeyPairSync("rsa", { modulusLength: 2048 })
        .privateKey.export({ format: "pem", type: "pkcs8" })
        .toString(),
    );
    const missing = join(dir, "missing.p8");
    const problems: string[] = [];
    readPush({ APNS_KEY_FILE: p8() }, problems);
    readPush(
      {
        APNS_KEY_FILE: rsa,
        APNS_KEY_ID: "ABC123DEFG",
        APNS_TEAM_ID: "D577WD6Z5U",
      },
      problems,
    );
    readPush(
      {
        APNS_KEY_FILE: missing,
        APNS_KEY_ID: "ABC123DEFG",
        APNS_TEAM_ID: "D577WD6Z5U",
      },
      problems,
    );
    expect(problems).toEqual([
      "APNS_KEY_ID is not a key id like ABC123DEFG",
      "APNS_TEAM_ID is not a team id like D577WD6Z5U",
      `APNS_KEY_FILE ${rsa} is not a P-256 key, as Apple's .p8 keys are`,
      `APNS_KEY_FILE ${missing} cannot be read (ENOENT)`,
    ]);
  });
});
