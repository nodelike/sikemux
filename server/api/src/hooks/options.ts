import { SOCKET_DEFAULTS, type SocketOptions } from "../sockets/socket.ts";

export const HOOKS_PATH = "/v1/hooks/live";

/** Every limit and timer of webhooks and their hosts' connections, so tests can shorten them. */
export interface HookOptions extends SocketOptions {
  maxSockets: number;
  messagesPerSecond: number;
  maxMessageBytes: number;
  hooksPerHost: number;
  requestsPerMinute: number;
  maxBodyBytes: number;
  /** Requests one host may have waiting, by count and by their bodies' bytes. */
  waitingPerHost: number;
  waitingBytesPerHost: number;
  /** How long a request waits for its host before it is dropped. */
  keptMs: number;
  /** How many requests may be sent ahead of the host finishing them. */
  deliveriesAhead: number;
  /** Above this many unsent bytes a connection is closed, to take its requests up again later. */
  dropBytes: number;
  /** A request the host could not handle comes again after this, doubling each time up to the last. */
  firstRetryMs: number;
  lastRetryMs: number;
  reconnectSpreadMs: number;
}

export const HOOK_OPTIONS: HookOptions = {
  ...SOCKET_DEFAULTS,
  messagesPerSecond: 50,
  maxMessageBytes: 4 * 1024,
  hooksPerHost: 100,
  requestsPerMinute: 60,
  maxBodyBytes: 1024 * 1024,
  waitingPerHost: 500,
  waitingBytesPerHost: 32 * 1024 * 1024,
  keptMs: 24 * 60 * 60_000,
  deliveriesAhead: 4,
  dropBytes: 16 * 1024 * 1024,
  firstRetryMs: 15_000,
  lastRetryMs: 30 * 60_000,
};
