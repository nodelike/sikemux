import {
  LiveCloseCode,
  type AccountEvent,
  type LiveDeviceMessage,
  type LiveHello,
  type LiveServerMessage,
  type RevokeReason,
} from "../generated/types.ts";

/** The parts of a WebSocket the live connection uses, the browser's and React Native's alike. */
export interface SocketLike {
  send(data: string): void;
  close(code?: number, reason?: string): void;
}

export interface SocketHandlers {
  message(data: unknown): void;
  closed(code: number): void;
}

export interface LiveClientDeps {
  url: string;
  connect(url: string, on: SocketHandlers): SocketLike;
  /** The hello answering the challenge's nonce, or null once signed out, which stops the connection. */
  hello(nonce: string): Promise<LiveHello | null>;
  /** A sign-in token, from past any cache when `fresh`, to renew a sign-in the server would let expire. */
  token(fresh: boolean): Promise<string | null>;
  /** Handles a frame of events: the id to acknowledge, and why this device is gone when it is. */
  events(events: AccountEvent[]): Promise<{ ack: number; gone?: RevokeReason }>;
  /** The server took the hello. */
  ready?(): void;
  /** The server pruned events this device never heard, so whatever it shows must be read again. */
  reset(): void;
  /** No longer on the account. Called once; the connection stays closed after it. */
  gone(reason: RevokeReason): void;
  random?: () => number;
}

const BASE_MS = 1000;
const MAX_MS = 60_000;
/** Until the server says how often it pings, assume its usual interval. */
const HEARTBEAT_MS = 25_000;
/** The server's limits count over a minute, so a device over one waits at least this long. */
const OVER_LIMIT_WAIT_MS = 30_000;
/** A sign-in is renewed this long before the server would close the connection for it. */
const RENEW_AHEAD_MS = 15_000;

/** Exponential backoff with half of it random, so devices that lost the server together do not return together. */
export function reconnectDelay(attempt: number, random: number): number {
  const ceiling = Math.min(MAX_MS, BASE_MS * 2 ** attempt);
  return Math.round(ceiling / 2 + (ceiling / 2) * random);
}

/** After a restart notice, wait as asked, plus up to a second so every device does not return at once. */
export function afterByeDelay(
  reconnectAfterMs: number,
  random: number,
): number {
  return reconnectAfterMs + Math.round(1000 * random);
}

/** `https://api` becomes `wss://api`, and the development server's `http` becomes `ws`. */
export function liveUrl(api: string): string {
  return `${api.replace(/^http/, "ws")}/v1/live`;
}

/** When a Clerk session token stops being accepted, in milliseconds since 1970. */
function tokenExpiry(token: string): number {
  const payload = (token.split(".")[1] ?? "")
    .replace(/-/g, "+")
    .replace(/_/g, "/");
  return (JSON.parse(atob(payload)) as { exp: number }).exp * 1000;
}

function parse(data: unknown): LiveServerMessage | undefined {
  if (typeof data !== "string") return undefined;
  try {
    const message = JSON.parse(data) as LiveServerMessage | null;
    return message &&
      typeof message === "object" &&
      typeof message.type === "string"
      ? message
      : undefined;
  } catch {
    return undefined;
  }
}

/** A device's connection to /v1/live: hears the account's changes as they happen, and learns when it is gone. */
export class LiveClient {
  private socket: SocketLike | undefined;
  private attempt = 0;
  private retrying: ReturnType<typeof setTimeout> | undefined;
  private silence: ReturnType<typeof setTimeout> | undefined;
  private renewing: ReturnType<typeof setTimeout> | undefined;
  private heartbeatMs = HEARTBEAT_MS;
  private byeMs: number | undefined;
  private running = false;
  private ended = false;
  private handling: Promise<void> = Promise.resolve();

  private readonly deps: LiveClientDeps;

  constructor(deps: LiveClientDeps) {
    this.deps = deps;
  }

  get connected(): boolean {
    return this.socket !== undefined;
  }

  start() {
    if (this.running || this.ended) return;
    this.running = true;
    this.attempt = 0;
    this.open();
  }

  /** Tries again now rather than when the backoff says, as when the network comes back. */
  nudge() {
    if (!this.running || this.socket) return;
    this.attempt = 0;
    this.open();
  }

  stop() {
    this.running = false;
    clearTimeout(this.retrying);
    this.clearTimers();
    const socket = this.socket;
    this.socket = undefined;
    socket?.close(1000, "stopped");
  }

  private clearTimers() {
    clearTimeout(this.silence);
    clearTimeout(this.renewing);
  }

  private random() {
    return (this.deps.random ?? Math.random)();
  }

  private open() {
    clearTimeout(this.retrying);
    let socket: SocketLike | undefined;
    const on: SocketHandlers = {
      message: (data) => {
        if (!socket || this.socket !== socket) return;
        this.heard();
        const message = parse(data);
        if (!message) return;
        const from = socket;
        this.handling = this.handling
          .then(() => this.handle(from, message))
          .catch((error: unknown) => {
            console.warn(
              "sikemux: the account connection could not handle a message",
              error,
            );
            this.drop(from);
          });
      },
      closed: (code) => {
        if (!socket || this.socket !== socket) return;
        this.socket = undefined;
        this.closed(code);
      },
    };
    try {
      socket = this.deps.connect(this.deps.url, on);
    } catch {
      this.closed(0);
      return;
    }
    this.socket = socket;
    this.heard();
  }

  /** Something arrived, so the connection is alive; silence for two heartbeats means it is not. */
  private heard() {
    clearTimeout(this.silence);
    const socket = this.socket;
    this.silence = setTimeout(() => {
      if (socket) this.drop(socket);
    }, this.heartbeatMs * 2);
  }

  private drop(socket: SocketLike) {
    if (this.socket !== socket) return;
    this.socket = undefined;
    socket.close(1000, "reconnecting");
    this.closed(0);
  }

  private send(socket: SocketLike, message: LiveDeviceMessage) {
    socket.send(JSON.stringify(message));
  }

  private async handle(socket: SocketLike, message: LiveServerMessage) {
    if (this.socket !== socket) return;
    switch (message.type) {
      case "challenge": {
        const hello = await this.deps.hello(message.nonce);
        if (this.socket !== socket) return;
        if (!hello) return this.stop();
        this.send(socket, hello);
        return;
      }
      case "ready":
        this.attempt = 0;
        this.heartbeatMs = message.heartbeatMs;
        this.heard();
        if (message.authExpiresAt)
          this.renewBefore(socket, Date.parse(message.authExpiresAt));
        this.deps.ready?.();
        return;
      case "events": {
        const { ack, gone } = await this.deps.events(message.events);
        if (this.socket === socket && ack > 0)
          this.send(socket, { type: "ack", id: ack });
        if (gone) this.end(gone);
        return;
      }
      case "reset":
        this.deps.reset();
        return;
      case "ping":
        this.send(socket, { type: "pong" });
        return;
      case "bye":
        this.byeMs = message.reconnectAfterMs;
        return;
      case "revoked":
        this.end(message.reason);
        return;
    }
  }

  private renewBefore(socket: SocketLike, expiry: number) {
    clearTimeout(this.renewing);
    this.renewing = setTimeout(
      () => void this.renew(socket, expiry),
      Math.max(1000, expiry - Date.now() - RENEW_AHEAD_MS),
    );
  }

  /** Clerk may still hand out the token the server already has; ask again past its cache if so. */
  private async renew(socket: SocketLike, expiry: number) {
    let token = await this.deps.token(false);
    if (token && tokenExpiry(token) <= expiry)
      token = await this.deps.token(true);
    if (!token || this.socket !== socket) return;
    this.send(socket, { type: "auth", token });
    this.renewBefore(socket, tokenExpiry(token));
  }

  private end(reason: RevokeReason) {
    if (this.ended) return;
    this.ended = true;
    this.stop();
    this.deps.gone(reason);
  }

  private closed(code: number) {
    this.clearTimers();
    if (!this.running) return;
    if (code === LiveCloseCode.revoked) return this.end("removed");
    if (code === LiveCloseCode.replaced) {
      this.running = false;
      return;
    }
    if (this.byeMs !== undefined) {
      const wait = afterByeDelay(this.byeMs, this.random());
      this.byeMs = undefined;
      this.reconnectIn(wait);
      return;
    }
    const wait = reconnectDelay(this.attempt, this.random());
    this.reconnectIn(
      code === LiveCloseCode.overLimit
        ? Math.max(wait, OVER_LIMIT_WAIT_MS)
        : wait,
    );
    this.attempt += 1;
  }

  private reconnectIn(wait: number) {
    clearTimeout(this.retrying);
    this.retrying = setTimeout(() => {
      if (this.running) this.open();
    }, wait);
  }
}
