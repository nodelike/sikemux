import { createHash, randomBytes } from "node:crypto";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { ShareCloseCode, type ShareServerMessage } from "@sikemux/protocol";
import type { KyselyPlugin } from "kysely";
import { afterAll, beforeAll, beforeEach, expect, it } from "vitest";
import WebSocket from "ws";

import type { Database } from "../src/db.ts";
import { shareMessage } from "../src/devices/signature.ts";
import { RateLimiter } from "../src/limits.ts";
import { attachShares, type Shares } from "../src/share/server.ts";
import {
  emptyTables,
  migratedDatabase,
  registered,
  signText,
  type TestDevice,
} from "./accounts.ts";
import { APP_ORIGIN, log, testApp } from "./support.ts";
import { verifier } from "./tokens.ts";

const ARJUN = "user_arjun";

let database: Database;
let drop: () => Promise<void>;
let host: TestDevice;

beforeAll(async () => {
  ({ database, drop } = await migratedDatabase());
});

beforeEach(async () => {
  await emptyTables(database);
  host = await registered(testApp(database), ARJUN, "host");
});

afterAll(() => drop());

/** Keeps the result of each read of a share a viewer makes until `released`. */
function heldReads() {
  let reach = () => {};
  let release = () => {};
  const reached = new Promise<void>((resolve) => (reach = resolve));
  const released = new Promise<void>((resolve) => (release = resolve));
  const plugin: KyselyPlugin = {
    transformQuery: ({ node }) => node,
    async transformResult({ result }) {
      const [row] = result.rows;
      if (row && "viewer_check" in row && "ended_reason" in row) {
        reach();
        await released;
      }
      return result;
    },
  };
  return { plugin, reached, release };
}

function connect(server: Server) {
  const { port } = server.address() as AddressInfo;
  const socket = new WebSocket(`ws://127.0.0.1:${port}/v1/share`);
  const waiting: ((message: ShareServerMessage) => void)[] = [];
  const unread: ShareServerMessage[] = [];
  socket.on("message", (data) => {
    const message = JSON.parse(data.toString()) as ShareServerMessage;
    const take = waiting.shift();
    if (take) take(message);
    else unread.push(message);
  });
  const closed = new Promise<number>((resolve) => socket.on("close", resolve));
  const next = () => {
    const message = unread.shift();
    return message
      ? Promise.resolve(message)
      : new Promise<ShareServerMessage>((resolve) => waiting.push(resolve));
  };
  return {
    closed,
    next,
    async until<T extends ShareServerMessage["type"]>(type: T) {
      for (;;) {
        const message = await next();
        if (message.type === type)
          return message as Extract<ShareServerMessage, { type: T }>;
      }
    },
    send: (message: object) => socket.send(JSON.stringify(message)),
  };
}

it("puts out a viewer who was joining without signing in when anyone was turned off", async () => {
  const held = heldReads();
  const server = createServer();
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const shares: Shares = attachShares(server, {
    database: { ...database, db: database.db.withPlugin(held.plugin) },
    verifier,
    limiter: new RateLimiter(),
    log,
    appOrigin: APP_ORIGIN,
  });
  try {
    const share = randomBytes(16).toString("base64url");
    const proof = randomBytes(32);
    const open = {
      type: "open",
      share,
      expiresAt: new Date(Date.now() + 3_600_000).toISOString(),
      viewers: createHash("sha256").update(proof).digest("hex"),
    };
    const owner = connect(server);
    const { nonce } = await owner.until("challenge");
    owner.send({
      type: "hello",
      role: "host",
      key: host.key,
      signature: signText(host, shareMessage(nonce, host.key)),
    });
    await owner.until("ready");
    owner.send({ ...open, anyone: true });
    await owner.until("opened");

    const viewer = connect(server);
    await viewer.until("challenge");
    viewer.send({
      type: "hello",
      role: "viewer",
      share,
      proof: proof.toString("base64url"),
    });
    await held.reached;
    owner.send({ ...open, anyone: false });
    await owner.until("opened");
    held.release();

    expect(
      await Promise.race([
        viewer.closed,
        viewer.until("ready").then(() => "joined"),
      ]),
    ).toBe(ShareCloseCode.unauthenticated);
    expect(shares.relay.rooms.get(share)?.viewers.size).toBe(0);
  } finally {
    await shares.stop();
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
});
