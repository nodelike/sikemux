import type { Server } from "node:http";

import type { Verifier } from "../auth.ts";
import type { Database } from "../db.ts";
import type { RateLimiter } from "../limits.ts";
import type { AccountWatcher } from "../live/listener.ts";
import type { Logger } from "../log.ts";
import { routeSockets } from "../sockets/upgrades.ts";
import { SHARE_OPTIONS, SHARE_PATH, type ShareOptions } from "./options.ts";
import { Relay } from "./relay.ts";
import { recheckShares, SHARE_SOCKET, ShareSocket } from "./socket.ts";
import { byeAll } from "../sockets/socket.ts";

/** The window of a released Mac app from before the GPUI one, which watches a shared chat from its own page. */
const MAC_ORIGIN = "tauri://localhost";

interface ShareServices {
  database: Database;
  verifier: Verifier;
  limiter: RateLimiter;
  log: Logger;
  appOrigin: string;
  options?: Partial<ShareOptions>;
}

export interface Shares extends AccountWatcher {
  relay: Relay;
  stop(): Promise<void>;
}

/** Serves /v1/share on the API's own HTTP server: hosts publish sealed chats, viewers watch them. */
export function attachShares(server: Server, services: ShareServices): Shares {
  const options = { ...SHARE_OPTIONS, ...services.options };
  const { database, limiter, log, appOrigin } = services;
  const relay = new Relay();
  const context = {
    db: database.db,
    verifier: services.verifier,
    relay,
    limiter,
    log,
    options,
  };
  const routed = routeSockets(server, {
    path: SHARE_PATH,
    kind: SHARE_SOCKET,
    origins: [appOrigin, MAC_ORIGIN],
    options,
    maxPayloadBytes: options.maxHostMessageBytes,
    limiter,
    log,
    open: () => relay.sockets.size,
    accept: (ws, ip) => new ShareSocket(ws, ip, context).start(),
  });

  const recheck = (userId?: string) => {
    recheckShares(context, userId).catch((error: unknown) =>
      log.error(
        { err: error },
        "rechecking shares after an account change failed",
      ),
    );
  };

  return {
    relay,
    changed: (userId) => recheck(userId),
    missed: () => recheck(),
    async stop() {
      routed.stop();
      await byeAll(relay.sockets, options.reconnectSpreadMs);
    },
  };
}
