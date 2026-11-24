import { createHmac, randomBytes } from "node:crypto";
import { afterAll, beforeAll, beforeEach, expect, it, vi } from "vitest";

import { ClerkRefusal } from "../src/account/clerk.ts";
import { sweepClerk } from "../src/account/clerk-sweeper.ts";
import { addressHash } from "../src/account/proofs.ts";
import { stripWorkSignIns } from "../src/account/sign-ins.ts";
import type { Database } from "../src/db.ts";
import {
  bodySha256,
  registrationMessage,
  requestMessage,
} from "../src/devices/signature.ts";
import {
  caller,
  emptyTables,
  migratedDatabase,
  newDevice,
  registered,
  signInMac,
  signText,
  type TestDevice,
} from "./accounts.ts";
import { FakeClerk } from "./clerk.ts";
import { RateLimiter } from "../src/limits.ts";
import { body, log, macCodes, testApp } from "./support.ts";
import { macToken, sessionToken } from "./tokens.ts";
import type * as Sentry from "../src/sentry.ts";
import { alertStaff } from "../src/sentry.ts";

vi.mock("../src/sentry.ts", async (original) => ({
  ...(await original<typeof Sentry>()),
  alertStaff: vi.fn(),
}));

const MAYA = "user_maya";
const PROOF_KEY = Buffer.alloc(32, 7);
const WEBHOOK_SECRET = randomBytes(24);

let database: Database;
let drop: () => Promise<void>;

beforeAll(async () => {
  ({ database, drop } = await migratedDatabase());
});

beforeEach(async () => {
  await emptyTables(database);
  await database.pool.query("truncate orgs, host_sign_ins cascade");
  vi.mocked(alertStaff).mockReset();
});

afterAll(() => drop());

/** Maya, admitted an hour ago with her own address, who just added her Acme one, which Acme counts as its own. */
async function mayaWithAcmeAddress() {
  const hourAgo = new Date(Date.now() - 3_600_000);
  const added = new Date();
  await database.db
    .insertInto("users")
    .values({ id: MAYA, admitted_at: hourAgo })
    .execute();
  const { id: orgId } = await database.db
    .insertInto("orgs")
    .values({ name: "Acme", created_by: MAYA })
    .returning("id")
    .executeTakeFirstOrThrow();
  await database.db
    .insertInto("org_domains")
    .values({
      org_id: orgId,
      domain: "acme.example",
      token: "a".repeat(22),
      added_by: MAYA,
      joining: "offered",
      verified_at: hourAgo,
      ours_since: hourAgo,
    })
    .execute();

  const clerk = new FakeClerk();
  clerk.users.set(MAYA, {
    primaryAddressId: "idn_own",
    addresses: [
      {
        id: "idn_own",
        email: "maya@okafor.example",
        signsIn: true,
        verified: true,
        addedAt: hourAgo,
      },
      {
        id: "idn_work",
        email: "maya@acme.example",
        signsIn: true,
        verified: true,
        addedAt: added,
      },
    ],
    externalAccounts: [
      {
        id: "eac_work",
        provider: "google",
        email: "maya@acme.example",
        addedAt: added,
      },
    ],
    phones: [],
    wallets: [],
    undated: [],
    accountsWithoutId: [],
    passwordSetAt: hourAgo,
  });
  return { clerk, added };
}

it("ends the session Maya opened with her Acme Google before the strip took it off", async () => {
  const { clerk, added } = await mayaWithAcmeAddress();
  clerk.userSessions.set(MAYA, [
    { id: "sess_own", createdAt: new Date(Date.now() - 3_600_000) },
    { id: "sess_work", createdAt: new Date(added.getTime() + 1) },
  ]);
  await stripWorkSignIns(database.db, clerk, null, log, MAYA);

  expect(clerk.users.get(MAYA)?.addresses.map((a) => a.id)).toEqual([
    "idn_own",
  ]);
  expect(clerk.revoked).toEqual(["sess_work"]);

  await stripWorkSignIns(database.db, clerk, null, log, MAYA);
  expect(clerk.revoked).toEqual(["sess_work"]);
});

it("takes off every way in the Acme session added, and the fold it started", async () => {
  const { clerk, added } = await mayaWithAcmeAddress();
  const inWindow = new Date(added.getTime() + 1);
  clerk.userSessions.set(MAYA, [{ id: "sess_work", createdAt: inWindow }]);
  const maya = clerk.users.get(MAYA);
  if (!maya) throw new Error("no Maya");
  maya.addresses.push({
    id: "idn_apple",
    email: "maya@lumen.example",
    signsIn: true,
    verified: true,
    addedAt: inWindow,
  });
  maya.externalAccounts.push({
    id: "eac_apple",
    provider: "apple",
    email: "maya@lumen.example",
    addedAt: inWindow,
  });
  maya.passwordSetAt = inWindow;
  await database.db
    .insertInto("account_folds")
    .values({
      into_user: MAYA,
      why: "seat",
      secret_hash: Buffer.alloc(32),
      expires_at: new Date(Date.now() + 600_000),
      started_session: "sess_work",
    })
    .execute();

  await stripWorkSignIns(database.db, clerk, null, log, MAYA);

  expect(maya.addresses.map((a) => a.id)).toEqual(["idn_own"]);
  expect(maya.externalAccounts).toEqual([]);
  expect(maya.passwordSetAt?.getTime()).toBeGreaterThan(inWindow.getTime());
  const fold = await database.db
    .selectFrom("account_folds")
    .select("expires_at")
    .executeTakeFirstOrThrow();
  expect(fold.expires_at.getTime()).toBeLessThanOrEqual(Date.now());
});

it("takes off a host the Acme session signed in by code, so its key no longer signs as Maya", async () => {
  const { clerk, added } = await mayaWithAcmeAddress();
  clerk.userSessions.set(MAYA, [
    { id: "sess_work", createdAt: new Date(added.getTime() + 1) },
  ]);
  const app = testApp(database);
  const work = await sessionToken(MAYA, {
    claims: { sid: "sess_work", fva: [0, -1] },
  });
  const host = newDevice();
  const started = await app.request("/v1/host-sign-ins", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      key: host.key,
      name: "Build box",
      platform: "linux",
      channel: "stable",
    }),
  });
  const { code, userCode } = await body(started, "HostSignIn");
  const approved = await caller(app)(
    `/v1/host-sign-ins/${userCode}/approve`,
    work,
    { method: "POST" },
  );
  expect(approved.status).toBe(204);
  const polled = await app.request("/v1/host-sign-ins/poll", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ code }),
  });
  const { nonce } = await body(polled, "HostSignInState");
  const finished = await app.request("/v1/host-sign-ins/finish", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      code,
      nonce,
      signature: signText(
        host,
        registrationMessage(nonce ?? "", MAYA, host.key),
      ),
    }),
  });
  expect(finished.status).toBe(201);
  expect((await signedAsHost(app, host)).status).toBe(200);

  await stripWorkSignIns(database.db, clerk, null, log, MAYA);

  expect((await signedAsHost(app, host)).status).toBe(401);
});

it("refuses the Acme session's token as soon as the strip ends it, before it expires", async () => {
  const { clerk, added } = await mayaWithAcmeAddress();
  clerk.userSessions.set(MAYA, [
    { id: "sess_work", createdAt: new Date(added.getTime() + 1) },
  ]);
  const app = testApp(database);
  const work = await sessionToken(MAYA, { claims: { sid: "sess_work" } });
  const challenge = () =>
    caller(app)("/v1/devices/challenge", work, { method: "POST" });
  expect((await challenge()).status).toBe(200);

  await stripWorkSignIns(database.db, clerk, null, log, MAYA);

  expect((await challenge()).status).toBe(401);
});

it("takes off a host a Mac signed in during the Acme session approved, and the host that one approved", async () => {
  const { clerk, added } = await mayaWithAcmeAddress();
  clerk.userSessions.set(MAYA, [
    { id: "sess_work", createdAt: new Date(added.getTime() + 1) },
  ]);
  const app = testApp(database);
  const mac = await signInMac(app, MAYA);
  expect(mac.response.status).toBe(201);
  const box = await approvedFromHost(app, mac);
  const second = await approvedFromHost(app, box);
  expect((await signedAsHost(app, second)).status).toBe(200);

  await stripWorkSignIns(database.db, clerk, null, log, MAYA);

  for (const host of [mac, box, second])
    expect((await signedAsHost(app, host)).status).toBe(401);
});

it("finishes no code a Mac signed in during the Acme session approved, though its phone was removed and came back", async () => {
  const { clerk, added } = await mayaWithAcmeAddress();
  clerk.userSessions.set(MAYA, [
    { id: "sess_work", createdAt: new Date(added.getTime() + 1) },
  ]);
  const app = testApp(database, undefined, { clerk });
  const phone = await registered(app, MAYA, "client", {
    sessionId: "sess_work",
  });
  const mac = await signInMac(app, MAYA);
  clerk.failing = true;
  await caller(app)(`/v1/devices/${phone.key}`, await sessionToken(MAYA), {
    method: "DELETE",
  });
  clerk.failing = false;
  await registered(app, MAYA, "client", {
    device: phone,
    sessionId: "sess_own",
  });
  const { finish } = await codeApprovedFromHost(app, mac);

  await stripWorkSignIns(database.db, clerk, null, log, MAYA);

  expect((await finish()).status).toBe(401);
});

it("adds no Mac while Clerk has yet to end the Acme session", async () => {
  const { clerk, added } = await mayaWithAcmeAddress();
  clerk.userSessions.set(MAYA, [
    { id: "sess_work", createdAt: new Date(added.getTime() + 1) },
  ]);
  const revoke = clerk.revokeSession.bind(clerk);
  clerk.revokeSession = () => Promise.reject(new Error("Clerk answered 500"));
  await expect(
    stripWorkSignIns(database.db, clerk, null, log, MAYA),
  ).rejects.toThrow();

  macCodes.set("code_unfinished", await macToken(MAYA));
  const refused = await testApp(database).request("/v1/mac-sign-ins", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      code: "code_unfinished",
      codeVerifier: "v".repeat(43),
      redirectUri: "http://127.0.0.1:53682/callback",
    }),
  });
  expect(refused.status).toBe(401);

  clerk.revokeSession = revoke;
  await stripWorkSignIns(database.db, clerk, null, log, MAYA);
  await database.db
    .updateTable("ended_sessions")
    .set({ ended_at: new Date(Date.now() - 3_600_000) })
    .execute();
  expect((await signInMac(testApp(database), MAYA)).response.status).toBe(201);
});

it("finishes a strip whose session Clerk stopped listing, on the sweep's timer", async () => {
  const { clerk, added } = await mayaWithAcmeAddress();
  clerk.userSessions.set(MAYA, [
    { id: "sess_work", createdAt: new Date(added.getTime() + 1) },
  ]);
  const revoke = clerk.revokeSession.bind(clerk);
  clerk.revokeSession = () => Promise.reject(new Error("Clerk answered 500"));
  await expect(
    stripWorkSignIns(database.db, clerk, null, log, MAYA),
  ).rejects.toThrow();
  clerk.revokeSession = revoke;
  clerk.userSessions.set(MAYA, []);
  await sweepClerk(database.db, clerk, null, log);
  expect(clerk.revoked).toEqual([]);

  await database.db
    .updateTable("ended_sessions")
    .set({ clerk_retry_at: new Date() })
    .execute();
  await sweepClerk(database.db, clerk, null, log);
  expect(clerk.revoked).toEqual(["sess_work"]);
  const removal = await database.db
    .selectFrom("sign_in_removals")
    .select("end_sessions_from")
    .executeTakeFirstOrThrow();
  expect(removal.end_sessions_from).toBeNull();
  await database.db
    .updateTable("ended_sessions")
    .set({ ended_at: new Date(Date.now() - 3_600_000) })
    .execute();
  expect((await signInMac(testApp(database), MAYA)).response.status).toBe(201);
});

it("runs the strip again for a removal still waiting after the pass ended its sessions", async () => {
  const { clerk, added } = await mayaWithAcmeAddress();
  clerk.userSessions.set(MAYA, [
    { id: "sess_work", createdAt: new Date(added.getTime() + 1) },
  ]);
  const revoke = clerk.revokeSession.bind(clerk);
  clerk.revokeSession = () => Promise.reject(new Error("Clerk answered 500"));
  await expect(
    stripWorkSignIns(database.db, clerk, null, log, MAYA),
  ).rejects.toThrow();
  clerk.revokeSession = revoke;
  const signIns = clerk.signIns.bind(clerk);
  clerk.signIns = () => Promise.reject(new Error("Clerk answered 500"));
  await database.db
    .updateTable("ended_sessions")
    .set({ clerk_retry_at: new Date() })
    .execute();
  await sweepClerk(database.db, clerk, null, log);
  expect(clerk.revoked).toEqual(["sess_work"]);
  const waiting = async () =>
    (
      await database.db
        .selectFrom("sign_in_removals")
        .select("end_sessions_from")
        .executeTakeFirstOrThrow()
    ).end_sessions_from;
  expect(await waiting()).not.toBeNull();

  clerk.signIns = signIns;
  await database.db
    .updateTable("sign_in_removals")
    .set({ rerun_at: new Date() })
    .execute();
  await sweepClerk(database.db, clerk, null, log);
  expect(await waiting()).toBeNull();
});

it("backs off running the strip again for a removal that keeps waiting", async () => {
  const { clerk, added } = await mayaWithAcmeAddress();
  clerk.userSessions.set(MAYA, [
    { id: "sess_work", createdAt: new Date(added.getTime() + 1) },
  ]);
  const revoke = clerk.revokeSession.bind(clerk);
  clerk.revokeSession = () => Promise.reject(new Error("Clerk answered 500"));
  await expect(
    stripWorkSignIns(database.db, clerk, null, log, MAYA),
  ).rejects.toThrow();
  clerk.revokeSession = revoke;
  let reads = 0;
  clerk.signIns = () => {
    reads++;
    return Promise.reject(new Error("Clerk answered 500"));
  };
  const rerun = () =>
    database.db
      .selectFrom("sign_in_removals")
      .select(["rerun_attempts", "rerun_at"])
      .executeTakeFirstOrThrow();

  for (const [attempts, minutes] of [
    [1, 1],
    [2, 2],
    [3, 4],
  ] as const) {
    await database.db
      .updateTable("sign_in_removals")
      .set({ rerun_at: new Date() })
      .execute();
    await sweepClerk(database.db, clerk, null, log);
    const row = await rerun();
    expect(row.rerun_attempts).toBe(attempts);
    expect(row.rerun_at.getTime() - Date.now()).toBeGreaterThan(
      (minutes - 0.5) * 60_000,
    );
  }
  const before = reads;
  await sweepClerk(database.db, clerk, null, log);
  expect(reads).toBe(before);
});

/** A host `approver` lets onto Maya's account with the code `sikemux login` showed on it. */
async function approvedFromHost(
  app: ReturnType<typeof testApp>,
  approver: TestDevice,
) {
  const { host, finish } = await codeApprovedFromHost(app, approver);
  expect((await finish()).status).toBe(201);
  return host;
}

async function codeApprovedFromHost(
  app: ReturnType<typeof testApp>,
  approver: TestDevice,
) {
  const host = newDevice();
  const post = (path: string, json: unknown) =>
    app.request(path, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(json),
    });
  const started = await post("/v1/host-sign-ins", {
    key: host.key,
    name: "Build box",
    platform: "linux",
    channel: "stable",
  });
  const { code, userCode } = await body(started, "HostSignIn");
  expect(
    (
      await signedAsHost(
        app,
        approver,
        `/v1/host-sign-ins/${userCode}/approve-from-host`,
      )
    ).status,
  ).toBe(200);
  const { nonce } = await body(
    await post("/v1/host-sign-ins/poll", { code }),
    "HostSignInState",
  );
  const finish = () =>
    post("/v1/host-sign-ins/finish", {
      code,
      nonce,
      signature: signText(
        host,
        registrationMessage(nonce ?? "", MAYA, host.key),
      ),
    });
  return { host, finish };
}

let signed = 0;

function signedAsHost(
  app: ReturnType<typeof testApp>,
  host: TestDevice,
  path = "/v1/devices/challenge",
) {
  const time = String(Math.floor(Date.now() / 1000) - signed++);
  const message = requestMessage(
    "host",
    "POST",
    path,
    time,
    bodySha256(new Uint8Array()),
  );
  return app.request(path, {
    method: "POST",
    headers: {
      authorization: `SikemuxHost ${host.key}.${time}.${signText(host, message)}`,
    },
  });
}

async function frozenAt() {
  const user = await database.db
    .selectFrom("users")
    .select("frozen_at")
    .where("id", "=", MAYA)
    .executeTakeFirstOrThrow();
  return user.frozen_at;
}

async function stripRetryAt() {
  const user = await database.db
    .selectFrom("users")
    .select("strip_retry_at")
    .where("id", "=", MAYA)
    .executeTakeFirstOrThrow();
  return user.strip_retry_at;
}

/** The sweep, once every removal still waiting is due. */
async function sweepRemovals(clerk: FakeClerk) {
  await database.db
    .updateTable("sign_in_removals")
    .set({ rerun_at: new Date() })
    .execute();
  await sweepClerk(database.db, clerk, null, log);
}

/** Clerk telling the API Maya's account changed, showing the addresses it lists with their times. */
function clerkUpdated(
  clerk: FakeClerk,
  addresses: { email: string; addedAt: Date; verified?: boolean }[],
) {
  const app = testApp(database, new RateLimiter(), {
    clerk,
    webhookSecret: `whsec_${WEBHOOK_SECRET.toString("base64")}`,
    addressProofKey: PROOF_KEY,
  });
  const id = `msg_${randomBytes(8).toString("hex")}`;
  const timestamp = Math.floor(Date.now() / 1000);
  const payload = JSON.stringify({
    type: "user.updated",
    object: "event",
    data: {
      id: MAYA,
      object: "user",
      email_addresses: addresses.map(({ email, addedAt, verified = true }) => ({
        email_address: email,
        created_at: addedAt.getTime(),
        verification: { status: verified ? "verified" : "unverified" },
      })),
    },
  });
  const signature = createHmac("sha256", WEBHOOK_SECRET)
    .update(`${id}.${timestamp}.${payload}`)
    .digest("base64");
  return app.request("/v1/webhooks/clerk", {
    method: "POST",
    body: payload,
    headers: {
      "content-type": "application/json",
      "svix-id": id,
      "svix-timestamp": String(timestamp),
      "svix-signature": `v1,${signature}`,
    },
  });
}

it("freezes the account when the Acme session took away every way in of Maya's own, after raising the staff issue", async () => {
  const { clerk, added } = await mayaWithAcmeAddress();
  clerk.userSessions.set(MAYA, [
    { id: "sess_work", createdAt: new Date(added.getTime() + 1) },
  ]);
  const phone = await registered(testApp(database), MAYA, "client", {
    sessionId: "sess_work",
  });
  const maya = clerk.users.get(MAYA);
  if (!maya) throw new Error("no Maya");
  maya.addresses = maya.addresses.filter((a) => a.id !== "idn_own");
  maya.primaryAddressId = "idn_work";

  vi.mocked(alertStaff).mockImplementationOnce(() => {
    throw new Error("Sentry is down");
  });
  await expect(
    stripWorkSignIns(database.db, clerk, PROOF_KEY, log, MAYA),
  ).rejects.toThrow("Sentry is down");
  expect(await frozenAt()).toBeNull();

  await stripWorkSignIns(database.db, clerk, PROOF_KEY, log, MAYA);

  expect(await frozenAt()).not.toBeNull();
  expect(alertStaff).toHaveBeenLastCalledWith(
    MAYA,
    expect.stringContaining("Froze an account"),
    expect.objectContaining({ why: "the work address is the last way in" }),
  );
  expect(clerk.revoked).toEqual(["sess_work"]);
  expect(maya.addresses.map((a) => a.id)).toEqual(["idn_work"]);
  const devices = await database.db
    .selectFrom("devices")
    .select("key")
    .where("key", "=", phone.key)
    .execute();
  expect(devices).toEqual([]);
});

it("freezes the account when Clerk refuses to take off a way the Acme session added", async () => {
  const { clerk, added } = await mayaWithAcmeAddress();
  const inWindow = new Date(added.getTime() + 1);
  clerk.userSessions.set(MAYA, [{ id: "sess_work", createdAt: inWindow }]);
  clerk.users
    .get(MAYA)
    ?.phones.push({ id: "idn_phone", signsIn: true, addedAt: inWindow });
  clerk.removePhone = () =>
    Promise.reject(new ClerkRefusal("Clerk answered 400"));

  await stripWorkSignIns(database.db, clerk, null, log, MAYA);

  expect(await frozenAt()).not.toBeNull();
  expect(alertStaff).toHaveBeenLastCalledWith(
    MAYA,
    expect.stringContaining("Froze an account"),
    expect.objectContaining({
      why: "Clerk refused to take off a way",
      added: expect.arrayContaining([{ kind: "phone", id: "idn_phone" }]),
    }),
  );
  expect(clerk.revoked).toEqual(["sess_work"]);
  const removal = await database.db
    .selectFrom("sign_in_removals")
    .select("end_sessions_from")
    .executeTakeFirstOrThrow();
  expect(removal.end_sessions_from).toBeNull();
});

it("freezes the account when Clerk refuses to take off the Acme Google itself", async () => {
  const { clerk, added } = await mayaWithAcmeAddress();
  clerk.userSessions.set(MAYA, [
    { id: "sess_work", createdAt: new Date(added.getTime() + 1) },
  ]);
  clerk.removeExternalAccount = () =>
    Promise.reject(new ClerkRefusal("Clerk answered 404"));

  await stripWorkSignIns(database.db, clerk, null, log, MAYA);

  expect(await frozenAt()).not.toBeNull();
  expect(alertStaff).toHaveBeenLastCalledWith(
    MAYA,
    expect.stringContaining("Froze an account"),
    expect.objectContaining({
      why: "Clerk refused to take off the work address",
      added: expect.arrayContaining([
        { kind: "google account", id: "eac_work" },
      ]),
    }),
  );
  expect(clerk.revoked).toEqual(["sess_work"]);
  const removal = await database.db
    .selectFrom("sign_in_removals")
    .select("end_sessions_from")
    .executeTakeFirstOrThrow();
  expect(removal.end_sessions_from).toBeNull();
});

it("freezes the account while Clerk fails to end the Acme session, when the Acme Google can't be taken off", async () => {
  const { clerk, added } = await mayaWithAcmeAddress();
  clerk.userSessions.set(MAYA, [
    { id: "sess_work", createdAt: new Date(added.getTime() + 1) },
  ]);
  clerk.removeExternalAccount = () =>
    Promise.reject(new ClerkRefusal("Clerk answered 404"));
  const revoke = clerk.revokeSession.bind(clerk);
  clerk.revokeSession = () => Promise.reject(new Error("Clerk answered 500"));

  await expect(
    stripWorkSignIns(database.db, clerk, null, log, MAYA),
  ).rejects.toThrow("Clerk answered 500");
  expect(await frozenAt()).not.toBeNull();

  clerk.revokeSession = revoke;
  await stripWorkSignIns(database.db, clerk, null, log, MAYA);
  expect(clerk.revoked).toEqual(["sess_work"]);
});

it("freezes the account while Clerk fails to end the Acme session, when the Acme address is the last way in", async () => {
  const { clerk, added } = await mayaWithAcmeAddress();
  clerk.userSessions.set(MAYA, [
    { id: "sess_work", createdAt: new Date(added.getTime() + 1) },
  ]);
  const maya = clerk.users.get(MAYA);
  if (!maya) throw new Error("no Maya");
  maya.addresses = maya.addresses.filter((a) => a.id !== "idn_own");
  maya.primaryAddressId = "idn_work";
  clerk.revokeSession = () => Promise.reject(new Error("Clerk answered 500"));

  await expect(
    stripWorkSignIns(database.db, clerk, PROOF_KEY, log, MAYA),
  ).rejects.toThrow("Clerk answered 500");
  expect(await frozenAt()).not.toBeNull();
});

it("refuses the Acme session while Clerk fails to take off the Acme Google, and nothing older", async () => {
  const { clerk, added } = await mayaWithAcmeAddress();
  clerk.userSessions.set(MAYA, [
    { id: "sess_own", createdAt: new Date(Date.now() - 3_600_000) },
    { id: "sess_work", createdAt: new Date(added.getTime() + 1) },
  ]);
  const removeAccount = clerk.removeExternalAccount.bind(clerk);
  clerk.removeExternalAccount = () =>
    Promise.reject(new Error("Clerk answered 500"));
  const app = testApp(database);
  const signedIn = async (sid: string) =>
    (
      await caller(app)(
        "/v1/devices/challenge",
        await sessionToken(MAYA, { claims: { sid } }),
        { method: "POST" },
      )
    ).status;

  await expect(
    stripWorkSignIns(database.db, clerk, null, log, MAYA),
  ).rejects.toThrow("Clerk answered 500");
  expect(await signedIn("sess_work")).toBe(401);
  expect(await signedIn("sess_own")).toBe(200);
  expect(await frozenAt()).toBeNull();

  clerk.userSessions.get(MAYA)?.push({
    id: "sess_again",
    createdAt: new Date(added.getTime() + 2),
  });
  await sweepRemovals(clerk);
  expect(await signedIn("sess_again")).toBe(401);
  expect(await signedIn("sess_own")).toBe(200);

  clerk.removeExternalAccount = removeAccount;
  await sweepRemovals(clerk);
  expect(clerk.users.get(MAYA)?.externalAccounts).toEqual([]);
  expect(clerk.users.get(MAYA)?.addresses.map((a) => a.id)).toEqual([
    "idn_own",
  ]);
  expect(clerk.revoked).toEqual(["sess_work", "sess_again"]);
  expect(await frozenAt()).toBeNull();
});

it("refuses the Acme session while Clerk fails to show the Acme address gone", async () => {
  const { clerk, added } = await mayaWithAcmeAddress();
  clerk.userSessions.set(MAYA, [
    { id: "sess_work", createdAt: new Date(added.getTime() + 1) },
  ]);
  const signIns = clerk.signIns.bind(clerk);
  let reads = 0;
  clerk.signIns = (userId) =>
    reads++ === 0
      ? signIns(userId)
      : Promise.reject(new Error("Clerk answered 500"));

  await expect(
    stripWorkSignIns(database.db, clerk, null, log, MAYA),
  ).rejects.toThrow("Clerk answered 500");

  const work = await sessionToken(MAYA, { claims: { sid: "sess_work" } });
  const response = await caller(testApp(database))(
    "/v1/devices/challenge",
    work,
    { method: "POST" },
  );
  expect(response.status).toBe(401);
});

it("freezes the account on the sweep's timer when Clerk failed to list its sessions, though the Acme address is the last way in", async () => {
  const { clerk, added } = await mayaWithAcmeAddress();
  clerk.userSessions.set(MAYA, [
    { id: "sess_work", createdAt: new Date(added.getTime() + 1) },
  ]);
  const maya = clerk.users.get(MAYA);
  if (!maya) throw new Error("no Maya");
  maya.addresses = maya.addresses.filter((a) => a.id !== "idn_own");
  maya.primaryAddressId = "idn_work";
  const sessions = clerk.sessions.bind(clerk);
  clerk.sessions = () => Promise.reject(new Error("Clerk answered 500"));
  await expect(
    stripWorkSignIns(database.db, clerk, PROOF_KEY, log, MAYA),
  ).rejects.toThrow("Clerk answered 500");

  clerk.sessions = sessions;
  await database.db
    .updateTable("users")
    .set({ strip_retry_at: new Date() })
    .execute();
  await sweepClerk(database.db, clerk, PROOF_KEY, log);

  expect(await frozenAt()).not.toBeNull();
  expect(clerk.revoked).toEqual(["sess_work"]);
  expect(await stripRetryAt()).toBeNull();
});

it("keeps a Mac Maya signed in from her own session while Clerk goes on failing to take off the Acme Google", async () => {
  const { clerk, added } = await mayaWithAcmeAddress();
  clerk.userSessions.set(MAYA, [
    { id: "sess_own", createdAt: new Date(Date.now() - 3_600_000) },
    { id: "sess_work", createdAt: new Date(added.getTime() + 1) },
  ]);
  clerk.removeExternalAccount = () =>
    Promise.reject(new Error("Clerk answered 500"));
  const app = testApp(database);
  await expect(
    stripWorkSignIns(database.db, clerk, null, log, MAYA),
  ).rejects.toThrow("Clerk answered 500");
  await database.db
    .updateTable("ended_sessions")
    .set({ ended_at: new Date(Date.now() - 3_600_000) })
    .execute();
  const mac = await signInMac(app, MAYA);
  expect(mac.response.status).toBe(201);

  await sweepRemovals(clerk);

  expect((await signedAsHost(app, mac)).status).toBe(200);
  expect((await signInMac(app, MAYA)).response.status).toBe(201);
});

it("keeps the sweep's timer a strip set while the sweep ran its own", async () => {
  const { clerk } = await mayaWithAcmeAddress();
  const signIns = clerk.signIns.bind(clerk);
  clerk.signIns = () => Promise.reject(new Error("Clerk answered 500"));
  const failedStrip = () =>
    expect(
      stripWorkSignIns(database.db, clerk, null, log, MAYA),
    ).rejects.toThrow("Clerk answered 500");
  await failedStrip();
  await database.db
    .updateTable("users")
    .set({ strip_retry_at: new Date() })
    .execute();
  const sessions = clerk.sessions.bind(clerk);
  let meanwhile: Promise<void> | null = null;
  clerk.sessions = async (userId) => {
    clerk.signIns = () => Promise.reject(new Error("Clerk answered 500"));
    meanwhile ??= failedStrip();
    await meanwhile;
    clerk.signIns = signIns;
    return sessions(userId);
  };
  clerk.signIns = signIns;

  await sweepClerk(database.db, clerk, null, log);

  expect(meanwhile).not.toBeNull();
  expect((await stripRetryAt())?.getTime()).toBeGreaterThan(Date.now());
});

it("runs the strip on the sweep's timer when Clerk failed to say what Maya signs in with", async () => {
  const { clerk, added } = await mayaWithAcmeAddress();
  clerk.userSessions.set(MAYA, [
    { id: "sess_work", createdAt: new Date(added.getTime() + 1) },
  ]);
  clerk.failing = true;
  await expect(
    stripWorkSignIns(database.db, clerk, null, log, MAYA),
  ).rejects.toThrow("Clerk answered 500");

  clerk.failing = false;
  await database.db
    .updateTable("users")
    .set({ strip_retry_at: new Date() })
    .where("strip_retry_at", "is not", null)
    .execute();
  await sweepClerk(database.db, clerk, null, log);

  expect(clerk.users.get(MAYA)?.addresses.map((a) => a.id)).toEqual([
    "idn_own",
  ]);
  expect(clerk.revoked).toEqual(["sess_work"]);
  expect(await stripRetryAt()).toBeNull();
});

it("takes off the Acme Google on the sweep's timer when Clerk fails after staff lifted the freeze", async () => {
  const { clerk } = await mayaWithAcmeAddress();
  const removeAccount = clerk.removeExternalAccount.bind(clerk);
  clerk.removeExternalAccount = () =>
    Promise.reject(new ClerkRefusal("Clerk answered 404"));
  await stripWorkSignIns(database.db, clerk, null, log, MAYA);
  await database.db.updateTable("users").set({ frozen_at: null }).execute();
  clerk.removeExternalAccount = () =>
    Promise.reject(new Error("Clerk answered 500"));
  await expect(
    stripWorkSignIns(database.db, clerk, null, log, MAYA),
  ).rejects.toThrow("Clerk answered 500");

  clerk.removeExternalAccount = removeAccount;
  await sweepRemovals(clerk);

  expect(clerk.users.get(MAYA)?.externalAccounts).toEqual([]);
  expect(await frozenAt()).toBeNull();
});

it("tells staff when Clerk lists a connected account without the id it is removed by", async () => {
  const { clerk } = await mayaWithAcmeAddress();
  const maya = clerk.users.get(MAYA);
  if (!maya) throw new Error("no Maya");
  maya.accountsWithoutId = ["google"];

  await stripWorkSignIns(database.db, clerk, null, log, MAYA);

  expect(alertStaff).toHaveBeenCalledWith(
    MAYA,
    expect.stringContaining("without the id"),
    { providers: ["google"] },
  );
});

it("keeps the ways Maya was admitted with though the Acme address was first seen before them", async () => {
  const { clerk, added } = await mayaWithAcmeAddress();
  clerk.failing = true;
  await clerkUpdated(clerk, [
    { email: "maya@acme.example", addedAt: new Date(Date.now() - 7_200_000) },
  ]);
  clerk.failing = false;
  clerk.userSessions.set(MAYA, [
    { id: "sess_work", createdAt: new Date(added.getTime() + 1) },
  ]);

  await stripWorkSignIns(database.db, clerk, PROOF_KEY, log, MAYA);

  expect(await frozenAt()).toBeNull();
  const maya = clerk.users.get(MAYA);
  expect(maya?.addresses.map((a) => a.id)).toEqual(["idn_own"]);
  expect(clerk.revoked).toEqual(["sess_work"]);
});

it("ends the Acme session when a late sighting shows the address was on the account before it was taken off and added again", async () => {
  const { clerk, added } = await mayaWithAcmeAddress();
  clerk.userSessions.set(MAYA, [
    { id: "sess_work", createdAt: new Date(added.getTime() + 1) },
  ]);
  const maya = clerk.users.get(MAYA);
  if (!maya) throw new Error("no Maya");
  const again = new Date(added.getTime() + 60_000);
  for (const address of maya.addresses)
    if (address.id === "idn_work") address.addedAt = again;
  for (const account of maya.externalAccounts) account.addedAt = again;

  expect(
    (
      await clerkUpdated(clerk, [
        {
          email: "maya@okafor.example",
          addedAt: new Date(added.getTime() - 3_600_000),
        },
        { email: "maya@acme.example", addedAt: again },
      ])
    ).status,
  ).toBe(204);
  expect(maya.addresses.map((a) => a.id)).toEqual(["idn_own"]);
  expect(clerk.revoked).toEqual([]);

  expect(
    (
      await clerkUpdated(clerk, [
        {
          email: "maya@okafor.example",
          addedAt: new Date(added.getTime() - 3_600_000),
        },
        { email: "maya@acme.example", addedAt: added },
      ])
    ).status,
  ).toBe(204);
  expect(clerk.revoked).toEqual(["sess_work"]);
});

/**
 * Clerk showed the Acme address on Maya's account while the strip could not run. Before it ran,
 * the Acme address and Maya's own went, and Kofi's Lumen address and session came.
 */
async function acmeDeletedBeforeTheStrip(
  clerk: FakeClerk,
  added: Date,
  { verified = true } = {},
) {
  clerk.failing = true;
  await clerkUpdated(clerk, [
    { email: "maya@okafor.example", addedAt: new Date(added.getTime() - 1) },
    { email: "maya@acme.example", addedAt: added, verified },
  ]);
  clerk.failing = false;
  const inWindow = new Date(added.getTime() + 1);
  clerk.userSessions.set(MAYA, [{ id: "sess_work", createdAt: inWindow }]);
  const maya = clerk.users.get(MAYA);
  if (!maya) throw new Error("no Maya");
  maya.addresses = [
    {
      id: "idn_lumen",
      email: "kofi@lumen.example",
      signsIn: true,
      verified: true,
      addedAt: inWindow,
    },
  ];
  maya.primaryAddressId = "idn_lumen";
  maya.externalAccounts = [];
}

it("sweeps, freezes and alerts when the Acme address was deleted before the strip's first run", async () => {
  const { clerk, added } = await mayaWithAcmeAddress();
  await acmeDeletedBeforeTheStrip(clerk, added);

  await stripWorkSignIns(database.db, clerk, PROOF_KEY, log, MAYA);

  expect(clerk.revoked).toEqual(["sess_work"]);
  expect(await frozenAt()).not.toBeNull();
  expect(alertStaff).toHaveBeenCalledWith(
    MAYA,
    expect.stringContaining("Froze an account"),
    expect.objectContaining({
      why: "nothing older would be left",
      added: [{ kind: "email address", id: "idn_lumen" }],
    }),
  );
});

it("sweeps nothing for a deleted Acme address that was never verified or connected", async () => {
  const { clerk, added } = await mayaWithAcmeAddress();
  await acmeDeletedBeforeTheStrip(clerk, added, { verified: false });

  await stripWorkSignIns(database.db, clerk, PROOF_KEY, log, MAYA);

  expect(clerk.revoked).toEqual([]);
  expect(await frozenAt()).toBeNull();
});

it("leaves a deleted Acme address alone when Acme claimed the domain after it appeared", async () => {
  const { clerk, added } = await mayaWithAcmeAddress();
  await database.db
    .updateTable("org_domains")
    .set({ ours_since: new Date(added.getTime() + 1) })
    .execute();
  await acmeDeletedBeforeTheStrip(clerk, added);

  await stripWorkSignIns(database.db, clerk, PROOF_KEY, log, MAYA);

  expect(clerk.revoked).toEqual([]);
  expect(await frozenAt()).toBeNull();
});

it("sweeps a deleted address proven as a work address at a domain no org claimed", async () => {
  const { clerk } = await mayaWithAcmeAddress();
  await database.db.deleteFrom("org_domains").execute();
  await database.db
    .insertInto("address_proofs")
    .values({
      address_hash: addressHash(PROOF_KEY, "maya@acme.example"),
      user_id: MAYA,
      domain: "acme.example",
      org_id: null,
      via: "google",
    })
    .execute();
  const added = new Date(Date.now() + 1000);
  await acmeDeletedBeforeTheStrip(clerk, added);

  await stripWorkSignIns(database.db, clerk, PROOF_KEY, log, MAYA);

  expect(clerk.revoked).toEqual(["sess_work"]);
  expect(await frozenAt()).not.toBeNull();
});
