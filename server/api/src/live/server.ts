import type { Server } from "node:http";

import type { Verifier } from "../auth.ts";
import type { Database } from "../db.ts";
import type { RateLimiter } from "../limits.ts";
import type { Logger } from "../log.ts";
import type { Pusher } from "../push/send.ts";
import { routeSockets } from "../sockets/upgrades.ts";
import { Hub } from "./hub.ts";
import {
  listenForEvents,
  type AccountWatcher,
  type Listener,
} from "./listener.ts";
import { LIVE_OPTIONS, type LiveOptions } from "./options.ts";
import { LIVE_SOCKET, LiveSocket } from "./socket.ts";

export interface LiveServices {
  database: Database;
  databaseUrl: string;
  verifier: Verifier;
  limiter: RateLimiter;
  log: Logger;
  appOrigin: string;
  pusher: Pusher;
  /** Told of every account change the hub hears of. */
  watchers?: AccountWatcher[];
  options?: Partial<LiveOptions>;
}

export interface Live {
  hub: Hub;
  /** Stops taking connections, says bye to every open one and waits for their cursors. */
  stop(): Promise<void>;
}

export const LIVE_PATH = "/v1/live";

/** Serves /v1/live on the API's own HTTP server, with one raw WebSocket per device. */
export function attachLive(server: Server, services: LiveServices): Live {
  const options = { ...LIVE_OPTIONS, ...services.options };
  const { database, limiter, log, appOrigin } = services;
  const hub = new Hub(options);
  const context = {
    db: database.db,
    verifier: services.verifier,
    hub,
    limiter,
    log,
    options,
    pusher: services.pusher,
  };
  const sockets = routeSockets(server, {
    path: LIVE_PATH,
    kind: LIVE_SOCKET,
    origins: [appOrigin],
    options,
    maxPayloadBytes: options.maxPayloadBytes,
    limiter,
    log,
    open: () => hub.open,
    accept: (ws, ip) => new LiveSocket(ws, ip, context).start(),
  });
  let stopping = false;

  const watchers = services.watchers ?? [];
  let listener: Listener | undefined = listenForEvents(
    services.databaseUrl,
    log,
    (userId) => {
      hub.wake(userId);
      for (const watcher of watchers) watcher.changed(userId);
    },
    () => {
      hub.wakeAll();
      for (const watcher of watchers) watcher.missed();
    },
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
      sockets.stop();
      clearInterval(sweep);
      await hub.closeAll(options.reconnectSpreadMs);
      await listener?.close();
      listener = undefined;
    },
  };
}
