import type { Server } from "node:http";

import type { Verifier } from "../auth.ts";
import type { Database } from "../db.ts";
import type { RateLimiter } from "../limits.ts";
import type { AccountWatcher } from "../live/listener.ts";
import type { Logger } from "../log.ts";
import { routeSockets } from "../sockets/upgrades.ts";
import type { HookHosts } from "./hosts.ts";
import { HOOK_OPTIONS, HOOKS_PATH, type HookOptions } from "./options.ts";
import { HOOK_SOCKET, HookSocket, recheckHookHosts } from "./socket.ts";
import { byeAll } from "../sockets/socket.ts";

export interface HookServices {
  database: Database;
  verifier: Verifier;
  limiter: RateLimiter;
  log: Logger;
  hosts: HookHosts;
  options?: Partial<HookOptions>;
}

export interface Hooks extends AccountWatcher {
  stop(): Promise<void>;
}

/** Serves /v1/hooks/live on the API's own HTTP server: hosts take their webhooks' requests. */
export function attachHooks(server: Server, services: HookServices): Hooks {
  const options = { ...HOOK_OPTIONS, ...services.options };
  const { database, limiter, log, hosts } = services;
  const context = {
    db: database.db,
    verifier: services.verifier,
    hosts,
    limiter,
    log,
    options,
  };
  const routed = routeSockets(server, {
    path: HOOKS_PATH,
    kind: HOOK_SOCKET,
    origins: [],
    options,
    maxPayloadBytes: options.maxMessageBytes,
    limiter,
    log,
    open: () => hosts.open,
    accept: (ws, ip) => new HookSocket(ws, ip, context).start(),
  });

  const recheck = (userId?: string) => {
    recheckHookHosts(context, userId).catch((error: unknown) =>
      log.error(
        { err: error },
        "rechecking webhook hosts after an account change failed",
      ),
    );
  };

  return {
    changed: (userId) => recheck(userId),
    missed: () => recheck(),
    async stop() {
      routed.stop();
      await byeAll(hosts.of(), options.reconnectSpreadMs);
    },
  };
}
