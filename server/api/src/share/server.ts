import type { IncomingMessage, Server } from "node:http";
import type { Duplex } from "node:stream";
import { ShareCloseCode } from "@sikemux/protocol";
import { WebSocketServer } from "ws";

import type { Verifier } from "../auth.ts";
import type { Database } from "../db.ts";
import { addressOf, type RateLimiter } from "../limits.ts";
import { refuse } from "../live/server.ts";
import type { Logger } from "../log.ts";
import { SHARE_OPTIONS, SHARE_PATH, type ShareOptions } from "./options.ts";
import { Relay } from "./relay.ts";
import { failedShareHelloKeys, ShareSocket } from "./socket.ts";

/** The Mac app's window, which watches a shared chat from its own page. */
const MAC_ORIGIN = "tauri://localhost";

export interface ShareServices {
  database: Database;
  verifier: Verifier;
  limiter: RateLimiter;
  log: Logger;
  appOrigin: string;
  options?: Partial<ShareOptions>;
}

export interface Shares {
  relay: Relay;
  stop(): Promise<void>;
}

/** Serves /v1/share on the API's own HTTP server: hosts publish sealed chats, viewers watch them. */
export function attachShares(server: Server, services: ShareServices): Shares {
  const options = { ...SHARE_OPTIONS, ...services.options };
  const { database, limiter, log, appOrigin } = services;
  const relay = new Relay();
  const sockets = new Set<ShareSocket>();
  const wss = new WebSocketServer({
    noServer: true,
    maxPayload: options.maxHostMessageBytes,
    perMessageDeflate: false,
    clientTracking: false,
  });
  const context = {
    db: database.db,
    verifier: services.verifier,
    relay,
    limiter,
    log,
    options,
  };
  let stopping = false;

  const onUpgrade = (
    request: IncomingMessage,
    socket: Duplex,
    head: Buffer,
  ) => {
    const path = new URL(request.url ?? "/", "http://api").pathname;
    if (path !== SHARE_PATH) return;
    if (stopping)
      return refuse(socket, log, 503, "unavailable", "The API is restarting.");
    const origin = request.headers.origin;
    if (origin !== undefined && origin !== appOrigin && origin !== MAC_ORIGIN)
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
    const [failedFromHere] = failedShareHelloKeys(ip);
    if (
      (failedFromHere &&
        limiter.exceeded(failedFromHere, options.failedHellosPerMinute)) ||
      !limiter.allow(`share-upgrade:${ip}`, options.upgradesPerMinute)
    )
      return refuse(
        socket,
        log,
        429,
        "rate_limited",
        "Too many connections; try again in a minute.",
      );

    wss.handleUpgrade(request, socket, head, (ws) => {
      if (relay.sockets >= options.maxSockets) {
        ws.close(ShareCloseCode.overloaded, "too many connections");
        return;
      }
      const share = new ShareSocket(ws, ip, context);
      sockets.add(share);
      ws.once("close", () => sockets.delete(share));
      share.start();
    });
  };
  server.on("upgrade", onUpgrade);

  return {
    relay,
    async stop() {
      if (stopping) return;
      stopping = true;
      const least = Math.min(1_000, options.reconnectSpreadMs);
      for (const socket of sockets)
        socket.bye(
          Math.round(
            least + Math.random() * (options.reconnectSpreadMs - least),
          ),
        );
    },
  };
}
