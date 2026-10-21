import { createPrivateKey } from "node:crypto";
import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";

import vector from "../../protocol/vectors/join.json" with { type: "json" };
import type { Database } from "../src/db.ts";
import { signedBy } from "../src/devices/signature.ts";
import { joinMessage, rawPublicKey, signTicket } from "../src/join/signer.ts";
import {
  caller,
  emptyTables,
  migratedDatabase,
  registered,
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
  await emptyTables(database);
  app = testApp(database);
});

afterAll(() => drop());

const phoneToken = (userId: string, sessionId = "sess_phone") =>
  sessionToken(userId, { claims: { sid: sessionId } });

async function join(phone: string, host: string, token: string) {
  return caller(app)(`/v1/devices/${phone}/join`, token, {
    method: "POST",
    json: { host },
  });
}

async function account(userId = "user_a") {
  const host = await registered(app, userId, "host");
  const phone = await registered(app, userId, "client");
  return { host, phone, token: await phoneToken(userId) };
}

describe("the join vector", () => {
  it("is the text the server signs, signed by the vector's key", () => {
    const { signature, ...fields } = vector.ticket;
    expect(joinMessage(fields)).toBe(vector.message);
    expect(signature).toBe(vector.signature);
    expect(signedBy(vector.publicKey, vector.message, vector.signature)).toBe(
      true,
    );
    const privateKey = createPrivateKey(vector.privateKeyPem);
    expect(rawPublicKey(privateKey)).toBe(vector.publicKey);
    expect(
      createPrivateKey({
        key: Buffer.concat([
          Buffer.from("302e020100300506032b657004220420", "hex"),
          Buffer.from(vector.secretKey, "hex"),
        ]),
        format: "der",
        type: "pkcs8",
      })
        .export({ format: "pem", type: "pkcs8" })
        .toString(),
    ).toBe(vector.privateKeyPem);
  });

  it("is what signing its fields makes", () => {
    const { keyId, account, host, phone, issuedAt } = vector.ticket;
    expect(
      signTicket(
        { keyId, privateKey: createPrivateKey(vector.privateKeyPem) },
        { account, host, phone },
        issuedAt * 1000 + 999,
      ),
    ).toEqual(vector.ticket);
  });
});

describe("POST /v1/devices/{key}/join", () => {
  it("signs a ten-minute ticket naming the account, the host and the phone", async () => {
    const { host, phone, token } = await account();
    const before = Math.floor(Date.now() / 1000);
    const response = await join(phone.key, host.key, token);
    expect(response.status).toBe(200);
    const ticket = await body(response, "JoinTicket");
    expect(ticket).toMatchObject({
      v: 1,
      keyId: vector.ticket.keyId,
      account: "user_a",
      host: host.key,
      phone: phone.key,
    });
    expect(ticket.issuedAt).toBeGreaterThanOrEqual(before);
    expect(ticket.issuedAt).toBeLessThanOrEqual(Math.ceil(Date.now() / 1000));
    expect(ticket.expiresAt - ticket.issuedAt).toBe(600);
    const { signature, ...fields } = ticket;
    expect(signedBy(vector.publicKey, joinMessage(fields), signature)).toBe(
      true,
    );

    const { rows } = await database.pool.query<{
      action: string;
      subject: string;
      detail: { host: string; keyId: string };
    }>(
      "select action, subject, detail from audit where action = 'join.issued'",
    );
    expect(rows).toEqual([
      {
        action: "join.issued",
        subject: phone.key,
        detail: { host: host.key, keyId: vector.ticket.keyId },
      },
    ]);
  });

  it("does not say whether another account's host or phone exists", async () => {
    const mine = await account("user_a");
    const theirs = await account("user_b");
    for (const [phone, host] of [
      [mine.phone.key, theirs.host.key],
      [theirs.phone.key, mine.host.key],
      [mine.host.key, mine.host.key],
      [mine.phone.key, mine.phone.key],
      ["0".repeat(64), mine.host.key],
    ] as const) {
      const response = await join(phone, host, mine.token);
      expect(response.status).toBe(404);
      expect((await body(response, "ApiError")).error.code).toBe("not_found");
    }
  });

  it("refuses a host or a phone taken off the account", async () => {
    const { host, phone, token } = await account();
    expect(
      (
        await caller(app)(`/v1/devices/${host.key}`, token, {
          method: "DELETE",
        })
      ).status,
    ).toBe(204);
    expect((await join(phone.key, host.key, token)).status).toBe(404);

    const other = await registered(app, "user_a", "host");
    expect(
      (
        await caller(app)(`/v1/devices/${phone.key}`, token, {
          method: "DELETE",
        })
      ).status,
    ).toBe(204);
    expect((await join(phone.key, other.key, token)).status).toBe(404);
  });

  it("links hosts for the Mac app's sign-in, as Macs on 0.5.3 ask, but joins no phone", async () => {
    const { host, phone } = await account();
    const other = await registered(app, "user_a", "host");
    const token = await macToken("user_a");
    expect((await join(other.key, host.key, token)).status).toBe(200);
    expect((await join(phone.key, host.key, token)).status).toBe(404);
  });

  it("refuses a request without a host key", async () => {
    const { phone, token } = await account();
    const response = await join(phone.key, "not a key", token);
    expect(response.status).toBe(400);
  });

  it("gives each phone 10 tickets a minute", async () => {
    const { host, phone, token } = await account();
    for (let i = 0; i < 10; i++)
      expect((await join(phone.key, host.key, token)).status).toBe(200);
    const limited = await join(phone.key, host.key, token);
    expect(limited.status).toBe(429);
    expect(limited.headers.get("retry-after")).toBe("60");

    const second = await registered(app, "user_a", "client", {
      sessionId: "sess_tablet",
    });
    expect(
      (
        await join(
          second.key,
          host.key,
          await phoneToken("user_a", "sess_tablet"),
        )
      ).status,
    ).toBe(200);
  });
});
