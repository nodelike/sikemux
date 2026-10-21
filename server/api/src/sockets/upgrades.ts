import { randomUUID } from "node:crypto";
import { STATUS_CODES, type IncomingMessage, type Server } from "node:http";
import type { Duplex } from "node:stream";
import type { ErrorCode } from "@sikemux/protocol";
import { WebSocketServer, type WebSocket } from "ws";

import { errorBody } from "../http.ts";
import { addressOf, type RateLimiter } from "../limits.ts";
import type { Logger } from "../log.ts";
import {
  failedHelloKeys,
  type SocketKind,
  type SocketOptions,
} from "./socket.ts";

/** One WebSocket path the API serves, and how it takes a connection. */
export interface SocketRoute {
  path: string;
  kind: SocketKind;
  /**
   * The browser pages that may connect. A connection without an Origin is not a browser, and
   * neither is one naming the API itself, which the phone's WebSocket sends and no page has.
   */
  origins: string[];
  options: SocketOptions & { maxSockets: number };
  maxPayloadBytes: number;
  limiter: RateLimiter;
  log: Logger;
  open(): number;
  accept(ws: WebSocket, ip: string): void;
}

export interface RoutedSockets {
  /** Refuses new connections from now on. */
  stop(): void;
}

/** Answers an upgrade with the API's error body, and logs its request id as every request's is. */
function refuse(
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

type Upgrade = (request: IncomingMessage, socket: Duplex, head: Buffer) => void;

const routers = new WeakMap<Server, Map<string, Upgrade>>();

/** The upgrade handler of each path on `server`; any other path is refused. */
function routesOf(server: Server, log: Logger): Map<string, Upgrade> {
  const known = routers.get(server);
  if (known) return known;
  const routes = new Map<string, Upgrade>();
  routers.set(server, routes);
  server.on(
    "upgrade",
    (request: IncomingMessage, socket: Duplex, head: Buffer) => {
      const path = new URL(request.url ?? "/", "http://api").pathname;
      const route = routes.get(path);
      if (route) route(request, socket, head);
      else refuse(socket, log, 404, "not_found", `No WebSocket at ${path}.`);
    },
  );
  return routes;
}

/** Serves `route` on the API's own HTTP server, beside Hono, checking each upgrade before it opens. */
export function routeSockets(
  server: Server,
  route: SocketRoute,
): RoutedSockets {
  const { options, limiter, log, kind } = route;
  const wss = new WebSocketServer({
    noServer: true,
    maxPayload: route.maxPayloadBytes,
    perMessageDeflate: false,
    clientTracking: false,
  });
  let stopping = false;
  routesOf(server, log).set(route.path, (request, socket, head) => {
    if (stopping)
      return refuse(socket, log, 503, "unavailable", "The API is restarting.");
    const origin = request.headers.origin;
    if (
      origin !== undefined &&
      !route.origins.includes(origin) &&
      URL.parse(origin)?.host !== request.headers.host
    )
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
    const [failedFromHere] = failedHelloKeys(kind.name, ip);
    if (
      (failedFromHere &&
        limiter.exceeded(failedFromHere, options.failedHellosPerMinute)) ||
      !limiter.allow(`${kind.name}-upgrade:${ip}`, options.upgradesPerMinute)
    )
      return refuse(
        socket,
        log,
        429,
        "rate_limited",
        "Too many connections; try again in a minute.",
      );
    wss.handleUpgrade(request, socket, head, (ws) => {
      if (route.open() >= options.maxSockets)
        ws.close(kind.codes.overloaded, "too many connections");
      else route.accept(ws, ip);
    });
  });
  return {
    stop() {
      stopping = true;
    },
  };
}
