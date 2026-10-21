import * as SecureStore from 'expo-secure-store';
import type { AccountEvent, LiveApp, RevokeReason } from '@protocol';
import { LiveClient, type SocketHandlers, type SocketLike } from '@protocol/live';

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
  /** Another device or host joined the account. */
  deviceAdded(key: string): void;
  /** The server took this phone's hello, so the account is reachable again. */
  ready?(): void;
  /** This phone is no longer on the account. Called once; the connection stays closed after it. */
  gone(reason: RevokeReason): void;
  random?: () => number;
};

/** How long after a device joins the account the person still hears of it; a phone back from days offline is not told of every one. */
const NEW_DEVICE_NEWS_MS = 10 * 60_000;

function isNews(at: string): boolean {
  return Date.now() - Date.parse(at) < NEW_DEVICE_NEWS_MS;
}

/** What a frame of events means to this phone. */
export function readEvents(
  events: AccountEvent[],
  ownKey: string,
  mark: number,
): { newest: number; handled: number; hostsChanged: boolean; added: string[]; gone?: RevokeReason } {
  let newest = 0;
  let handled = mark;
  let hostsChanged = false;
  const added: string[] = [];
  let gone: RevokeReason | undefined;
  for (const event of [...events].sort((a, b) => a.id - b.id)) {
    newest = Math.max(newest, event.id);
    if (event.id <= mark) continue;
    handled = event.id;
    if (event.type === 'account.deleted') gone = 'account_deleted';
    else if (event.type === 'device.revoked' && event.key === ownKey) gone ??= event.reason ?? 'removed';
    else {
      if (event.type === 'device.added' && event.key && event.key !== ownKey && isNews(event.at)) added.push(event.key);
      if (event.role === 'host') hostsChanged = true;
    }
  }
  return { newest, handled, hostsChanged, added, gone };
}

/** This phone's connection to the account: hears changes as they happen and learns when it was removed. */
export class LiveAccount extends LiveClient {
  constructor(deps: LiveDeps) {
    let ownKey = '';
    let mark: number | undefined;
    super({
      url: deps.url,
      connect: deps.connect,
      token: () => deps.token(),
      ready: deps.ready,
      reset: deps.hostsChanged,
      gone: deps.gone,
      random: deps.random,
      hello: async (nonce) => {
        const [key, signature, token] = await Promise.all([deps.key(), deps.sign(nonce), deps.token()]);
        if (!token) return null;
        ownKey = key;
        return { type: 'hello', role: 'client', key, signature, token, ...(deps.app ? { app: deps.app } : {}) };
      },
      events: async (events) => {
        mark ??= await deps.cursor.load();
        const read = readEvents(events, ownKey, mark);
        if (read.handled > mark) {
          await deps.cursor.save(read.handled);
          mark = read.handled;
        }
        if (read.hostsChanged) deps.hostsChanged();
        for (const key of read.added) deps.deviceAdded(key);
        return { ack: read.newest, gone: read.gone };
      },
    });
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
