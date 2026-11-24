import {
  generateKeyPairSync,
  randomUUID,
  sign,
  type KeyObject,
} from "node:crypto";
import type { Device, DeviceRegistration, DeviceRole } from "@sikemux/protocol";
import { expect } from "vitest";

import { openDatabase, type Database } from "../src/db.ts";
import {
  bodySha256,
  liveMessage,
  registrationMessage,
  requestMessage,
} from "../src/devices/signature.ts";
import { migrate, readMigrations } from "../src/migrations.ts";
import { body, freshDatabase, log, macCodes } from "./support.ts";
import { macToken, sessionToken } from "./tokens.ts";

export interface TestDevice {
  key: string;
  secret: KeyObject;
}

export function newDevice(): TestDevice {
  const { publicKey, privateKey } = generateKeyPairSync("ed25519");
  return {
    key: publicKey
      .export({ format: "der", type: "spki" })
      .subarray(12)
      .toString("hex"),
    secret: privateKey,
  };
}

export function signText(device: TestDevice, text: string): string {
  return sign(null, Buffer.from(text), device.secret).toString("hex");
}

export function signLive(device: TestDevice, nonce: string): string {
  return signText(device, liveMessage(nonce, device.key));
}

const signedRequests = new Set<string>();

/**
 * A host's `authorization` for one request. The API takes a signed request once, so a repeat of one
 * is signed for an earlier second.
 */
export function hostAuthorization(
  host: TestDevice,
  method: string,
  path: string,
  content = "",
): string {
  const sha = bodySha256(new TextEncoder().encode(content));
  let second = Math.floor(Date.now() / 1000);
  let signedRequest: string;
  do {
    const time = String(second--);
    const message = requestMessage("host", method, path, time, sha);
    signedRequest = `SikemuxHost ${host.key}.${time}.${signText(host, message)}`;
  } while (signedRequests.has(signedRequest));
  signedRequests.add(signedRequest);
  return signedRequest;
}

/** A migrated database of its own for one test file. */
export async function migratedDatabase(): Promise<{
  database: Database;
  drop(): Promise<void>;
}> {
  const fresh = await freshDatabase();
  const database = openDatabase(fresh.url, log);
  await migrate(
    database.pool,
    await readMigrations(new URL("../migrations", import.meta.url).pathname),
    log,
  );
  return {
    database,
    async drop() {
      await database.close();
      await fresh.drop();
    },
  };
}

export async function emptyTables(database: Database) {
  await database.pool.query(
    "truncate users, devices, challenges, audit, events, removed_devices cascade",
  );
}

interface Requester {
  request(path: string, init: RequestInit): Response | Promise<Response>;
}

export function caller(app: Requester) {
  return (
    path: string,
    token: string,
    init: { method?: string; json?: unknown } = {},
  ) =>
    Promise.resolve(
      app.request(path, {
        method: init.method ?? "GET",
        headers: {
          authorization: `Bearer ${token}`,
          "content-type": "application/json",
        },
        ...(init.json === undefined ? {} : { body: JSON.stringify(init.json) }),
      }),
    );
}

export async function challenge(app: Requester, token: string) {
  const response = await caller(app)("/v1/devices/challenge", token, {
    method: "POST",
  });
  expect(response.status).toBe(200);
  return (await body(response, "Challenge")).nonce;
}

export async function registration(
  app: Requester,
  device: TestDevice,
  userId: string,
  token: string,
  fields: Partial<DeviceRegistration> = {},
): Promise<DeviceRegistration> {
  const nonce = fields.nonce ?? (await challenge(app, token));
  const json: DeviceRegistration = {
    key: device.key,
    role: "host",
    name: "Studio Mac",
    platform: "macos",
    channel: "stable",
    nonce,
    signature: signText(device, registrationMessage(nonce, userId, device.key)),
    ...fields,
  };
  if (json.role === "client" && !("channel" in fields)) delete json.channel;
  return json;
}

/** Adds a host as a Mac's browser sign-in does, or a client with a session token for `sessionId`. */
export async function register(
  app: Requester,
  userId: string,
  role: DeviceRole,
  { device = newDevice(), sessionId = "sess_phone" } = {},
): Promise<TestDevice & { response: Response }> {
  if (role === "host") return signInMac(app, userId, { device });
  const token = await sessionToken(userId, { claims: { sid: sessionId } });
  const json = await registration(app, device, userId, token, {
    role,
    platform: "ios",
    name: "iPhone",
  });
  const response = await caller(app)("/v1/devices", token, {
    method: "POST",
    json,
  });
  return { ...device, response };
}

export async function registered(
  app: Requester,
  userId: string,
  role: DeviceRole,
  options: { device?: TestDevice; sessionId?: string } = {},
): Promise<TestDevice> {
  const { response, ...device } = await register(app, userId, role, options);
  expect([200, 201]).toContain(response.status);
  (await body(response as Response, "Device")) satisfies Device;
  return device;
}

export interface EventRow {
  id: number;
  user_id: string;
  type: string;
  subject: string | null;
  subject_role: string | null;
  reason: string | null;
}

export async function events(database: Database): Promise<EventRow[]> {
  const { rows } = await database.pool.query<EventRow>(
    "select id::int, user_id, type, subject, subject_role, reason from events order by id",
  );
  return rows;
}

/** A Mac's browser sign-in for `userId`: the code it hands the API, and the host it adds. */
export async function signInMac(
  app: Requester,
  userId: string,
  { device = newDevice(), code = `code_${randomUUID()}` } = {},
): Promise<TestDevice & { response: Response }> {
  macCodes.set(code, await macToken(userId));
  const post = (path: string, json: unknown) =>
    Promise.resolve(
      app.request(path, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(json),
      }),
    );
  const started = await post("/v1/mac-sign-ins", {
    code,
    codeVerifier: "v".repeat(43),
    redirectUri: "http://127.0.0.1:53682/callback",
  });
  expect(started.status).toBe(200);
  const { account, nonce } = await body(started, "MacSignInState");
  const response = await post("/v1/mac-sign-ins/finish", {
    key: device.key,
    name: "Studio Mac",
    platform: "macos",
    channel: "stable",
    nonce,
    signature: signText(
      device,
      registrationMessage(nonce, account, device.key),
    ),
  });
  return { ...device, response };
}
