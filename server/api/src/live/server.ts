import { randomUUID } from "node:crypto";
import { STATUS_CODES, type IncomingMessage, type Server } from "node:http";
import type { Duplex } from "node:stream";
import { LiveCloseCode, type ErrorCode } from "@sikemux/protocol";
import { WebSocketServer } from "ws";

import type { Verifier } from "../auth.ts";
import type { Database } from "../db.ts";
import { errorBody } from "../http.ts";
import { addressOf, type RateLimiter } from "../limits.ts";
import type { Logger } from "../log.ts";
import type { Pusher } from "../push/send.ts";
import { Hub } from "./hub.ts";
import { listenForEvents, type Listener } from "./listener.ts";
import { SHARE_PATH } from "../share/options.ts";
import { LIVE_OPTIONS, type LiveOptions } from "./options.ts";
import { failedHelloKeys, LiveSocket } from "./socket.ts";

export interface LiveServices {
  database: Database;
  databaseUrl: string;
  verifier: Verifier;
  limiter: RateLimiter;
  log: Logger;
  appOrigin: string;
  pusher: Pusher;
  options?: Partial<LiveOptions>;
}

export interface Live {
  hub: Hub;
  /** Stops taking connections, says bye to every open one and waits for their cursors. */
  stop(): Promise<void>;
}

export const LIVE_PATH = "/v1/live";

/** Answers an upgrade with the API's error body, and logs its request id as every request's is. */
export function refuse(
  socket: Duplex,
  log: Logger,
  status: number,
  code: ErrorCode,
  message: string,
) {
  const requestId = randomUUID();
  log.info({ requestId, status }, "refused a WebSocket upgrade");
  const text = JSON.stringify(errorBody(requestId, code, message));
  socket.end(
    `HTTP/1.1 ${status} ${STATUS_CODES[status]}\r\n` +
      "Connection: close\r\n" +
      "Content-Type: application/json\r\n" +
      (status === 429 ? "Retry-After: 60\r\n" : "") +
      `Content-Length: ${Buffer.byteLength(text)}\r\n\r\n${text}`,
  );
}

/** Serves /v1/live on the API's own HTTP server, beside Hono, with one raw WebSocket per device. */
export function attachLive(server: Server, services: LiveServices): Live {
  const options = { ...LIVE_OPTIONS, ...services.options };
  const { database, limiter, log, appOrigin } = services;
  const hub = new Hub(options);
  const wss = new WebSocketServer({
    noServer: true,
    maxPayload: options.maxPayloadBytes,
    perMessageDeflate: false,
    clientTracking: false,
  });
  const context = {
    db: database.db,
    verifier: services.verifier,
    hub,
    limiter,
    log,
    options,
    pusher: services.pusher,
  };
  let stopping = false;

  const onUpgrade = (
    request: IncomingMessage,
    socket: Duplex,
    head: Buffer,
  ) => {
    const path = new URL(request.url ?? "/", "http://api").pathname;
    if (path === SHARE_PATH) return;
    if (path !== LIVE_PATH)
      return refuse(socket, log, 404, "not_found", `No WebSocket at ${path}.`);
    if (stopping)
      return refuse(socket, log, 503, "unavailable", "The API is restarting.");
    const origin = request.headers.origin;
    if (origin !== undefined && origin !== appOrigin)
      return refuse(
        socket,
        log,
        403,
        "forbidden",
        "Only the web app may connect from a browser.",
      );
    const ip = addressOf(
      request.headers["x-forwarded-for"],
      request.socket.remoteAddress,
    );
    const [failedFromHere] = failedHelloKeys(ip);
    if (
      (failedFromHere &&
        limiter.exceeded(failedFromHere, options.failedHellosPerMinute)) ||
      !limiter.allow(`live-upgrade:${ip}`, options.upgradesPerMinute)
    )
      return refuse(
        socket,
        log,
        429,
        "rate_limited",
        "Too many connections; try again in a minute.",
      );

    wss.handleUpgrade(request, socket, head, (ws) => {
      if (hub.open >= options.maxSockets) {
        ws.close(LiveCloseCode.overloaded, "too many connections");
        return;
      }
      new LiveSocket(ws, ip, context).start();
    });
  };
  server.on("upgrade", onUpgrade);

  let listener: Listener | undefined = listenForEvents(
    services.databaseUrl,
    log,
    (userId) => hub.wake(userId),
    () => hub.wakeAll(),
  );
  const sweep = setInterval(() => {
    hub
      .sweep(database.db, log)
      .catch((error: unknown) =>
        log.warn({ err: error }, "sweeping live connections failed"),
      );
  }, options.sweepMs);

  return {
    hub,
    async stop() {
      if (stopping) return;
      stopping = true;
      clearInterval(sweep);
      await hub.closeAll(options.reconnectSpreadMs);
      await listener?.close();
      listener = undefined;
    },
  };
}
