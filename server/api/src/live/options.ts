import type { SocketOptions } from "../sockets/socket.ts";

/** Every limit and timer of the live connection, so tests can shorten them. */
export interface LiveOptions extends SocketOptions {
  /** Every connection is closed after this, so each proof is fresh at least this often. */
  maxAgeMs: number;
  /** Notifications for one account within this window wake its connections once. */
  coalesceMs: number;
  /** How often every connected account is checked for events a lost notification hid. */
  sweepMs: number;
  /** A connection writes its acknowledged cursor at most this often. */
  cursorFlushMs: number;
  maxSockets: number;
  socketsPerUser: number;
  messagesPerSecond: number;
  maxPayloadBytes: number;
  /** Above this many unsent bytes a connection gets no more backlog until it drains. */
  pauseBytes: number;
  /** Above this many unsent bytes a connection is closed, to resume later from its cursor. */
  dropBytes: number;
  eventsPerFrame: number;
  /** How many frames may be sent ahead of the device's last acknowledgement. */
  framesAhead: number;
  /** Pushes one host may have waiting on the platforms at once; more are answered throttled. */
  pushesInFlight: number;
  /** A restart asks each device to wait a random time up to this before reconnecting. */
  reconnectSpreadMs: number;
}

export const LIVE_OPTIONS: LiveOptions = {
  challengeMs: 30_000,
  helloTimeoutMs: 10_000,
  heartbeatMs: 25_000,
  silenceMs: 60_000,
  maxAgeMs: 12 * 60 * 60_000,
  authGraceMs: 30_000,
  coalesceMs: 50,
  sweepMs: 60_000,
  cursorFlushMs: 1_000,
  maxSockets: 20_000,
  socketsPerUser: 20,
  upgradesPerMinute: 60,
  failedHellosPerMinute: 10,
  messagesPerSecond: 20,
  maxPayloadBytes: 8 * 1024,
  pauseBytes: 256 * 1024,
  dropBytes: 1024 * 1024,
  eventsPerFrame: 200,
  framesAhead: 2,
  reconnectSpreadMs: 15_000,
  pushesInFlight: 32,
};
