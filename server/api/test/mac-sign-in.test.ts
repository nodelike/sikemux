import { afterAll, beforeAll, beforeEach, expect, it } from "vitest";

import { sweepClerk } from "../src/account/clerk-sweeper.ts";
import type { Database } from "../src/db.ts";
import { registerDevice } from "../src/devices/register.ts";
import { approveHost } from "../src/host/codes.ts";
import {
  bodySha256,
  registrationMessage,
  requestMessage,
} from "../src/devices/signature.ts";
import {
  caller,
  challenge,
  emptyTables,
  migratedDatabase,
  newDevice,
  registered,
  registration,
  signInMac,
  signText,
  type TestDevice,
} from "./accounts.ts";
import { FakeClerk } from "./clerk.ts";
import { body, log, macCodes, testApp } from "./support.ts";
import { macToken, sessionToken } from "./tokens.ts";

const MAYA = "user_maya";

let database: Database;
let drop: () => Promise<void>;

beforeAll(async () => {
  ({ database, drop } = await migratedDatabase());
});

beforeEach(async () => {
  await emptyTables(database);
  macCodes.clear();
});

afterAll(() => drop());

function post(app: ReturnType<typeof testApp>, path: string, json: unknown) {
  return app.request(path, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(json),
  });
}

const SIGN_IN = {
  codeVerifier: "v".repeat(43),
  redirectUri: "http://127.0.0.1:53682/callback",
};

let skew = 0;

function signed(
  app: ReturnType<typeof testApp>,
  host: TestDevice,
  path: string,
  method = "GET",
) {
  const time = String(Math.floor(Date.now() / 1000) - skew++);
  const message = requestMessage(
    "host",
    method,
    path,
    time,
    bodySha256(new Uint8Array()),
  );
  return app.request(path, {
    method,
    headers: {
      authorization: `SikemuxHost ${host.key}.${time}.${signText(host, message)}`,
    },
  });
}

it("trades a sign-in code only once, and refuses one Clerk does not know", async () => {
  const app = testApp(database);
  const mac = await signInMac(app, MAYA, { code: "code_once" });
  expect(mac.response.status).toBe(201);

  macCodes.set("code_once", await macToken(MAYA));
  const again = await post(app, "/v1/mac-sign-ins", {
    ...SIGN_IN,
    code: "code_once",
  });
  expect(again.status).toBe(401);

  const junk = await post(app, "/v1/mac-sign-ins", {
    ...SIGN_IN,
    code: "code_junk",
  });
  expect(junk.status).toBe(401);
});

it("counts toward everyone's limit only the sign-ins that reach Clerk", async () => {
  const app = testApp(database);
  for (let address = 1; address <= 13; address++)
    for (let i = 0; i < 10; i++) {
      const junk = await app.request("/v1/mac-sign-ins", {
        method: "POST",
        headers: {
          "content-type": "application/json",
          "x-forwarded-for": `203.0.113.${address}`,
        },
        body: "{}",
      });
      expect(junk.status).toBe(400);
    }
  expect((await signInMac(app, MAYA)).response.status).toBe(201);
});

it("keeps a code good when Clerk could not be reached", async () => {
  let down = true;
  const app = testApp(database, undefined, {
    macSignIn: {
      async accessToken({ code }) {
        if (down) throw new Error("Clerk is down");
        return code === "code_retry" ? macToken(MAYA) : null;
      },
    },
  });
  const sign = () =>
    post(app, "/v1/mac-sign-ins", { ...SIGN_IN, code: "code_retry" });
  expect((await sign()).status).toBe(503);
  down = false;
  expect((await sign()).status).toBe(200);
});

it("refuses a Mac sign-in until a host removal has signed the account out and its codes ran out", async () => {
  const clerk = new FakeClerk();
  const app = testApp(database, undefined, { clerk });
  const mac = await signInMac(app, MAYA);
  macCodes.set("code_before", await macToken(MAYA));
  const { nonce } = await body(
    await post(app, "/v1/mac-sign-ins", { ...SIGN_IN, code: "code_before" }),
    "MacSignInState",
  );
  clerk.failing = true;
  const removed = await app.request(`/v1/devices/${mac.key}`, {
    method: "DELETE",
    headers: { authorization: `Bearer ${await sessionToken(MAYA)}` },
  });
  expect(removed.status).toBe(204);

  const trade = async () => {
    const code = `code_${crypto.randomUUID()}`;
    macCodes.set(code, await macToken(MAYA));
    return post(app, "/v1/mac-sign-ins", { ...SIGN_IN, code });
  };
  const stuck = await trade();
  expect(stuck.status).toBe(401);
  expect((await body(stuck, "ApiError")).error.message).toBe(
    "This account was just signed out somewhere. Try again in 11 minutes.",
  );
  expect(stuck.headers.get("retry-after")).toBe("660");
  const device = newDevice();
  const finish = await post(app, "/v1/mac-sign-ins/finish", {
    key: device.key,
    name: "Studio Mac",
    platform: "macos",
    channel: "stable",
    nonce,
    signature: signText(device, registrationMessage(nonce, MAYA, device.key)),
  });
  expect(finish.status).toBe(401);

  clerk.failing = false;
  await database.pool.query(
    "update host_removal_sign_outs set clerk_retry_at = now()",
  );
  await sweepClerk(database.db, clerk, null, log);
  expect((await trade()).status).toBe(401);

  await database.pool.query(
    "update host_removal_sign_outs set done_at = now() - interval '12 minutes'",
  );
  expect((await trade()).status).toBe(200);
});

it("refuses a Mac sign-in after a phone's removal ended its session, until its codes ran out", async () => {
  const clerk = new FakeClerk();
  const app = testApp(database, undefined, { clerk });
  const phone = await registered(app, MAYA, "client", {
    sessionId: "sess_phone",
  });
  const removed = await app.request(`/v1/devices/${phone.key}`, {
    method: "DELETE",
    headers: { authorization: `Bearer ${await sessionToken(MAYA)}` },
  });
  expect(removed.status).toBe(204);
  expect(clerk.revoked).toEqual(["sess_phone"]);

  macCodes.set("code_after", await macToken(MAYA));
  const refused = await post(app, "/v1/mac-sign-ins", {
    ...SIGN_IN,
    code: "code_after",
  });
  expect(refused.status).toBe(401);
  expect(refused.headers.get("retry-after")).toBe("660");

  await database.pool.query(
    "update removed_devices set clerk_revoked_at = now() - interval '12 minutes'",
  );
  expect((await signInMac(app, MAYA)).response.status).toBe(201);
});

it("lets no session approve a `sikemux login` code while the wait runs, and a host still can", async () => {
  const clerk = new FakeClerk();
  const app = testApp(database, undefined, { clerk });
  const mac = await signInMac(app, MAYA);
  const phone = await registered(app, MAYA, "client", {
    sessionId: "sess_phone",
  });
  const fresh = await sessionToken(MAYA, { claims: { fva: [0, -1] } });
  await caller(app)(`/v1/devices/${phone.key}`, fresh, { method: "DELETE" });

  const box = newDevice();
  const { userCode } = await body(
    await post(app, "/v1/host-sign-ins", {
      key: box.key,
      name: "Build box",
      platform: "linux",
      channel: "stable",
    }),
    "HostSignIn",
  );
  const fromSession = await caller(app)(
    `/v1/host-sign-ins/${userCode}/approve`,
    fresh,
    { method: "POST" },
  );
  expect(fromSession.status).toBe(401);
  const fromHost = await signed(
    app,
    mac,
    `/v1/host-sign-ins/${userCode}/approve-from-host`,
    "POST",
  );
  expect(fromHost.status).toBe(200);
});

it("finishes no `sikemux login` code a session approved before a removal while the wait runs, and one a host approved", async () => {
  const clerk = new FakeClerk();
  const app = testApp(database, undefined, { clerk });
  const mac = await signInMac(app, MAYA);
  const phone = await registered(app, MAYA, "client", {
    sessionId: "sess_phone",
  });
  const fresh = await sessionToken(MAYA, { claims: { fva: [0, -1] } });
  const started = async () => {
    const box = newDevice();
    const sign = await body(
      await post(app, "/v1/host-sign-ins", {
        key: box.key,
        name: "Build box",
        platform: "linux",
        channel: "stable",
      }),
      "HostSignIn",
    );
    return { box, ...sign };
  };
  const bySession = await started();
  expect(
    (
      await caller(app)(
        `/v1/host-sign-ins/${bySession.userCode}/approve`,
        fresh,
        { method: "POST" },
      )
    ).status,
  ).toBe(204);
  const byHost = await started();
  expect(
    (
      await signed(
        app,
        mac,
        `/v1/host-sign-ins/${byHost.userCode}/approve-from-host`,
        "POST",
      )
    ).status,
  ).toBe(200);
  await caller(app)(`/v1/devices/${phone.key}`, fresh, { method: "DELETE" });

  const finish = async ({ box, code }: Awaited<ReturnType<typeof started>>) => {
    const { nonce } = await body(
      await post(app, "/v1/host-sign-ins/poll", { code }),
      "HostSignInState",
    );
    return post(app, "/v1/host-sign-ins/finish", {
      code,
      nonce,
      signature: signText(box, registrationMessage(nonce ?? "", MAYA, box.key)),
    });
  };
  expect((await finish(bySession)).status).toBe(401);
  expect((await finish(byHost)).status).toBe(201);
});

it("adds a Mac only with the challenge its traded code got", async () => {
  const app = testApp(database);
  const device = newDevice();
  const nonce = await challenge(app, await sessionToken(MAYA));
  const fromToken = await post(app, "/v1/mac-sign-ins/finish", {
    key: device.key,
    name: "Studio Mac",
    platform: "macos",
    channel: "stable",
    nonce,
    signature: signText(device, registrationMessage(nonce, MAYA, device.key)),
  });
  expect(fromToken.status).toBe(400);

  macCodes.set("code_maya", await macToken(MAYA));
  const started = await post(app, "/v1/mac-sign-ins", {
    ...SIGN_IN,
    code: "code_maya",
  });
  const state = await body(started, "MacSignInState");
  expect(state.account).toBe(MAYA);
  const session = await sessionToken(MAYA);
  const throughBearer = await caller(app)("/v1/devices", session, {
    method: "POST",
    json: await registration(app, device, MAYA, session, {
      nonce: state.nonce,
      role: "client",
      platform: "ios",
    }),
  });
  expect(throughBearer.status).toBe(400);
});

it("lets a Mac removed after its request was checked approve no code and register no phone", async () => {
  const app = testApp(database);
  const mac = await signInMac(app, MAYA);
  const box = newDevice();
  const { userCode } = await body(
    await post(app, "/v1/host-sign-ins", {
      key: box.key,
      name: "Build box",
      platform: "linux",
      channel: "stable",
    }),
    "HostSignIn",
  );
  const session = await sessionToken(MAYA);
  const phone = await registration(app, newDevice(), MAYA, session, {
    role: "client",
    platform: "ios",
    name: "Maya's iPhone",
  });
  await caller(app)(`/v1/devices/${mac.key}`, session, { method: "DELETE" });

  const asMac = {
    userId: MAYA,
    via: "host" as const,
    deviceKey: mac.key,
    expiresAt: new Date(Date.now() + 60_000),
  };
  await expect(approveHost(database.db, asMac, userCode)).rejects.toMatchObject(
    { status: 401 },
  );
  await expect(registerDevice(database.db, asMac, phone)).rejects.toMatchObject(
    { status: 401 },
  );
});

it("lets a removed Mac reach nothing", async () => {
  const app = testApp(database);
  const mac = await signInMac(app, MAYA);
  expect(mac.response.status).toBe(201);
  expect((await signed(app, mac, "/v1/devices")).status).toBe(200);

  const removed = await caller(app)(
    `/v1/devices/${mac.key}`,
    await sessionToken(MAYA),
    { method: "DELETE" },
  );
  expect(removed.status).toBe(204);

  expect((await signed(app, mac, "/v1/devices")).status).toBe(401);
  expect((await signed(app, mac, "/v1/vault/uses")).status).toBe(401);
});

it("holds no Mac sign-in after a phone removed with no Clerk signs in again", async () => {
  const app = testApp(database);
  const phone = await registered(app, MAYA, "client", {
    sessionId: "sess_phone",
  });
  const removed = await caller(app)(
    `/v1/devices/${phone.key}`,
    await sessionToken(MAYA),
    { method: "DELETE" },
  );
  expect(removed.status).toBe(204);
  await registered(app, MAYA, "client", {
    device: phone,
    sessionId: "sess_phone_again",
  });
  expect((await signInMac(app, MAYA)).response.status).toBe(201);
});

it("lets a Mac's sign-in token of any age reach nothing", async () => {
  const app = testApp(database);
  const mac = await signInMac(app, MAYA);
  const token = await macToken(MAYA);
  const call = caller(app);
  const refusals = [
    await call("/v1/devices", token),
    await call("/v1/devices/challenge", token, { method: "POST" }),
    await call("/v1/devices", token, {
      method: "POST",
      json: await registration(
        app,
        newDevice(),
        MAYA,
        await sessionToken(MAYA),
      ),
    }),
    await call(`/v1/devices/${newDevice().key}/join`, token, {
      method: "POST",
      json: { host: mac.key },
    }),
    await call("/v1/vault/uses", token),
  ];
  for (const refused of refusals) {
    expect(refused.status).toBe(401);
    expect((await body(refused, "ApiError")).error.message).toBe(
      "Update Sikemux to sign in.",
    );
  }
});
