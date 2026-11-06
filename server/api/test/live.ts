import type { AddressInfo } from "node:net";
import type { Server } from "node:http";
import { serve } from "@hono/node-server";
import type { LiveDeviceMessage, LiveServerMessage } from "@sikemux/protocol";
import WebSocket from "ws";

import type { Database } from "../src/db.ts";
import { RateLimiter } from "../src/limits.ts";
import type { LiveOptions } from "../src/live/options.ts";
import { attachLive, type Live } from "../src/live/server.ts";
import { Pusher, type PusherOptions } from "../src/push/send.ts";
import { signLive, type TestDevice } from "./accounts.ts";
import { APP_ORIGIN, log, protocol, testApp } from "./support.ts";
import { sessionToken, verifier } from "./tokens.ts";

export interface Running {
  url: string;
  live: Live;
  app: ReturnType<typeof testApp>;
  limiter: RateLimiter;
  stop(): Promise<void>;
}

/** The API on a real port with /v1/live attached, as the server runs it. */
export async function runApi(
  database: Database,
  options: Partial<LiveOptions> = {},
  push: Partial<Pick<PusherOptions, "providers" | "limits" | "sleep">> = {},
): Promise<Running> {
  const limiter = new RateLimiter();
  const app = testApp(database, limiter);
  const server = await new Promise<Server>((resolve) => {
    const started = serve(
      { fetch: app.fetch, hostname: "127.0.0.1", port: 0 },
      () => resolve(started as Server),
    ) as Server;
  });
  const live = attachLive(server, {
    database,
    databaseUrl: String(database.pool.options.connectionString),
    verifier,
    limiter,
    log,
    appOrigin: APP_ORIGIN,
    pusher: new Pusher({
      db: database.db,
      log,
      limiter,
      providers: {},
      ...push,
    }),
    options: { coalesceMs: 10, ...options },
  });
  const { port } = server.address() as AddressInfo;
  await new Promise((resolve) => setTimeout(resolve, 50));
  return {
    url: `ws://127.0.0.1:${port}/v1/live`,
    live,
    app,
    limiter,
    async stop() {
      await live.stop();
      server.closeAllConnections();
      await new Promise((resolve) => server.close(resolve));
    },
  };
}

/** A live connection from the test's side, keeping every message in order. */
export class Peer {
  readonly messages: LiveServerMessage[] = [];
  readonly closed: Promise<number>;
  private readonly socket: WebSocket;
  private waiters: (() => void)[] = [];
  private read = 0;
  closeCode: number | undefined;

  constructor(url: string, headers: Record<string, string> = {}) {
    this.socket = new WebSocket(url, { headers });
    this.socket.on("message", (data) => {
      const value: unknown = JSON.parse(data.toString());
      const result = protocol.validate("LiveServerMessage", value);
      if (!result.ok)
        throw new Error(`not a server message: ${result.problems.join("; ")}`);
      this.messages.push(result.value);
      this.notify();
    });
    this.closed = new Promise((resolve) => {
      this.socket.on("close", (code) => {
        this.closeCode = code;
        this.notify();
        resolve(code);
      });
      this.socket.on("error", () => undefined);
    });
  }

  private notify() {
    const waiters = this.waiters;
    this.waiters = [];
    for (const wake of waiters) wake();
  }

  async next(timeoutMs = 3_000): Promise<LiveServerMessage> {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
      const message = this.messages[this.read];
      if (message) {
        this.read += 1;
        return message;
      }
      if (this.closeCode !== undefined)
        throw new Error(`closed with ${this.closeCode} before another message`);
      if (Date.now() > deadline) throw new Error("no message in time");
      await new Promise<void>((resolve) => {
        this.waiters.push(resolve);
        setTimeout(resolve, 50);
      });
    }
  }

  async until<T extends LiveServerMessage["type"]>(
    type: T,
    timeoutMs = 3_000,
  ): Promise<Extract<LiveServerMessage, { type: T }>> {
    for (;;) {
      const message = await this.next(timeoutMs);
      if (message.type === type)
        return message as Extract<LiveServerMessage, { type: T }>;
    }
  }

  /** Every message that arrives within `ms`, without waiting for any in particular. */
  async quiet(ms = 200): Promise<LiveServerMessage[]> {
    await new Promise((resolve) => setTimeout(resolve, ms));
    const unread = this.messages.slice(this.read);
    this.read = this.messages.length;
    return unread;
  }

  async challenge(): Promise<string> {
    const first = await this.next();
    if (first.type !== "challenge") throw new Error("expected a challenge");
    return first.nonce;
  }

  send(message: LiveDeviceMessage | Record<string, unknown>) {
    this.raw(JSON.stringify(message));
  }

  raw(text: string | Buffer) {
    this.socket.send(text);
  }

  close() {
    this.socket.close();
  }
}

/** Whether the upgrade itself was refused, and with which HTTP status. */
export function upgradeStatus(
  url: string,
  headers: Record<string, string> = {},
): Promise<number> {
  return new Promise((resolve) => {
    const socket = new WebSocket(url, { headers });
    socket.on("unexpected-response", (_request, response) => {
      resolve(response.statusCode ?? 0);
      socket.terminate();
    });
    socket.on("open", () => {
      resolve(101);
      socket.close();
    });
    socket.on("error", () => undefined);
  });
}

export async function hostPeer(
  url: string,
  device: TestDevice,
  headers: Record<string, string> = {},
): Promise<Peer> {
  const peer = new Peer(url, headers);
  const nonce = await peer.challenge();
  peer.send({
    type: "hello",
    role: "host",
    key: device.key,
    signature: signLive(device, nonce),
  });
  return peer;
}

export async function clientPeer(
  url: string,
  device: TestDevice,
  userId: string,
  { sessionId = "sess_phone", headers = {} as Record<string, string> } = {},
): Promise<Peer> {
  const peer = new Peer(url, headers);
  const nonce = await peer.challenge();
  peer.send({
    type: "hello",
    role: "client",
    key: device.key,
    signature: signLive(device, nonce),
    token: await sessionToken(userId, { claims: { sid: sessionId } }),
  });
  return peer;
}

export async function webPeer(
  url: string,
  userId: string,
  token?: string,
): Promise<Peer> {
  const peer = new Peer(url, { origin: APP_ORIGIN });
  await peer.challenge();
  peer.send({
    type: "hello",
    role: "web",
    token:
      token ?? (await sessionToken(userId, { claims: { azp: APP_ORIGIN } })),
  });
  return peer;
}
