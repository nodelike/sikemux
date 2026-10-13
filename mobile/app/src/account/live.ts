import * as SecureStore from 'expo-secure-store';
import {
  LiveCloseCode,
  type AccountEvent,
  type LiveApp,
  type LiveDeviceMessage,
  type LiveServerMessage,
  type RevokeReason,
} from '@protocol';

/** The parts of React Native's WebSocket the live connection uses. */
export type SocketLike = {
  send(data: string): void;
  close(code?: number, reason?: string): void;
};

export type SocketHandlers = { message(data: unknown): void; closed(code: number): void };

export type Cursor = { load(): Promise<number>; save(id: number): Promise<void> };

export type LiveDeps = {
  url: string;
  connect(url: string, on: SocketHandlers): SocketLike;
  /** This phone's key and its signature over the server's challenge. */
  key(): Promise<string>;
  sign(nonce: string): Promise<string>;
  /** A Clerk session token, or null once signed out. */
  token(): Promise<string | null>;
  app?: LiveApp;
  cursor: Cursor;
  /** A host joined, changed or left the account: the list of hosts is stale. */
  hostsChanged(): void;
  /** The server took this phone's hello, so the account is reachable again. */
  ready?(): void;
  /** This phone is no longer on the account. Called once; the connection stays closed after it. */
  gone(reason: RevokeReason): void;
  random?: () => number;
};

const BASE_MS = 1000;
const MAX_MS = 60_000;
/** Until the server says how often it pings, assume its usual interval. */
const HEARTBEAT_MS = 25_000;
/** The server's limits count over a minute, so a phone over one waits at least this long. */
const OVER_LIMIT_WAIT_MS = 30_000;

/** Exponential backoff with half of it random, so phones that lost the server together do not return together. */
export function reconnectDelay(attempt: number, random: number): number {
  const ceiling = Math.min(MAX_MS, BASE_MS * 2 ** attempt);
  return Math.round(ceiling / 2 + (ceiling / 2) * random);
}

/** After a restart notice, wait as asked, plus up to a second so every device does not return at once. */
export function afterByeDelay(reconnectAfterMs: number, random: number): number {
  return reconnectAfterMs + Math.round(1000 * random);
}

/** What a frame of events means to this phone. */
export function readEvents(
  events: AccountEvent[],
  ownKey: string,
  mark: number,
): { newest: number; handled: number; hostsChanged: boolean; gone?: RevokeReason } {
  let newest = 0;
  let handled = mark;
  let hostsChanged = false;
  let gone: RevokeReason | undefined;
  for (const event of [...events].sort((a, b) => a.id - b.id)) {
    newest = Math.max(newest, event.id);
    if (event.id <= mark) continue;
    handled = event.id;
    if (event.type === 'account.deleted') gone = 'account_deleted';
    else if (event.type === 'device.revoked' && event.key === ownKey) gone ??= event.reason ?? 'removed';
    else if (event.role === 'host') hostsChanged = true;
  }
  return { newest, handled, hostsChanged, gone };
}

/** This phone's connection to the account: hears changes as they happen and learns when it was removed. */
export class LiveAccount {
  private socket?: SocketLike;
  private attempt = 0;
  private retrying?: ReturnType<typeof setTimeout>;
  private silence?: ReturnType<typeof setTimeout>;
  private heartbeatMs = HEARTBEAT_MS;
  private byeMs?: number;
  private running = false;
  private ended = false;
  private ownKey?: string;
  private mark?: number;
  private handling: Promise<void> = Promise.resolve();

  constructor(private readonly deps: LiveDeps) {}

  get connected(): boolean {
    return this.socket !== undefined;
  }

  start() {
    if (this.running || this.ended) return;
    this.running = true;
    this.attempt = 0;
    this.open();
  }

  /** Tries again now rather than when the backoff says, as when the phone's network comes back. */
  nudge() {
    if (!this.running || this.socket) return;
    this.attempt = 0;
    this.open();
  }

  stop() {
    this.running = false;
    clearTimeout(this.retrying);
    clearTimeout(this.silence);
    const socket = this.socket;
    this.socket = undefined;
    socket?.close(1000, 'stopped');
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
            console.warn('sikemux: the account connection could not handle a message', error);
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
      if (socket && this.socket === socket) this.drop(socket);
    }, this.heartbeatMs * 2);
  }

  private drop(socket: SocketLike) {
    if (this.socket !== socket) return;
    this.socket = undefined;
    socket.close(1000, 'reconnecting');
    this.closed(0);
  }

  private send(socket: SocketLike, message: LiveDeviceMessage) {
    socket.send(JSON.stringify(message));
  }

  private async handle(socket: SocketLike, message: LiveServerMessage) {
    if (this.socket !== socket) return;
    switch (message.type) {
      case 'challenge': {
        const [key, signature, token] = await Promise.all([this.deps.key(), this.deps.sign(message.nonce), this.deps.token()]);
        if (this.socket !== socket) return;
        if (!token) {
          this.stop();
          return;
        }
        this.ownKey = key;
        this.send(socket, { type: 'hello', role: 'client', key, signature, token, ...(this.deps.app ? { app: this.deps.app } : {}) });
        return;
      }
      case 'ready':
        this.attempt = 0;
        this.heartbeatMs = message.heartbeatMs;
        this.heard();
        this.deps.ready?.();
        return;
      case 'events':
        return this.events(socket, message.events);
      case 'reset':
        this.deps.hostsChanged();
        return;
      case 'ping':
        this.send(socket, { type: 'pong' });
        return;
      case 'bye':
        this.byeMs = message.reconnectAfterMs;
        return;
      case 'revoked':
        this.end(message.reason);
        return;
    }
  }

  private async events(socket: SocketLike, events: AccountEvent[]) {
    this.mark ??= await this.deps.cursor.load();
    const read = readEvents(events, this.ownKey ?? '', this.mark);
    if (read.handled > this.mark) {
      await this.deps.cursor.save(read.handled);
      this.mark = read.handled;
    }
    if (this.socket === socket && read.newest > 0) this.send(socket, { type: 'ack', id: read.newest });
    if (read.hostsChanged) this.deps.hostsChanged();
    if (read.gone) this.end(read.gone);
  }

  private end(reason: RevokeReason) {
    if (this.ended) return;
    this.ended = true;
    this.stop();
    this.deps.gone(reason);
  }

  private closed(code: number) {
    clearTimeout(this.silence);
    if (!this.running) return;
    if (code === LiveCloseCode.revoked) return this.end('removed');
    if (code === LiveCloseCode.replaced) {
      this.running = false;
      return;
    }
    if (this.byeMs !== undefined) {
      const wait = afterByeDelay(this.byeMs, this.random());
      this.byeMs = undefined;
      this.scheduleReconnect(wait);
      return;
    }
    const wait = reconnectDelay(this.attempt, this.random());
    this.scheduleReconnect(code === LiveCloseCode.overLimit ? Math.max(wait, OVER_LIMIT_WAIT_MS) : wait);
    this.attempt += 1;
  }

  private scheduleReconnect(wait: number) {
    clearTimeout(this.retrying);
    this.retrying = setTimeout(() => {
      if (this.running) this.open();
    }, wait);
  }
}

function parse(data: unknown): LiveServerMessage | undefined {
  if (typeof data !== 'string') return undefined;
  try {
    const message = JSON.parse(data) as LiveServerMessage | null;
    return message && typeof message === 'object' && typeof message.type === 'string' ? message : undefined;
  } catch {
    return undefined;
  }
}

const CURSOR_ITEM = 'sikemux.account-event';

/** The newest account event this phone has handled, kept so a reconnect never applies one twice. */
export const savedCursor: Cursor = {
  async load() {
    const stored = await SecureStore.getItemAsync(CURSOR_ITEM);
    const id = stored ? Number(stored) : 0;
    return Number.isSafeInteger(id) && id > 0 ? id : 0;
  },
  save: (id) => SecureStore.setItemAsync(CURSOR_ITEM, String(id)),
};

export function forgetCursor(): Promise<void> {
  return SecureStore.deleteItemAsync(CURSOR_ITEM);
}

let current: LiveAccount | undefined;

/** Runs one live connection at a time; a new one replaces the last. */
export function runLive(live: LiveAccount) {
  current?.stop();
  current = live;
}

export function stopLive() {
  current?.stop();
}

/** `https://api` becomes `wss://api`, and the development server's `http` becomes `ws`. */
export function liveUrl(api: string): string {
  return `${api.replace(/^http/, 'ws')}/v1/live`;
}
