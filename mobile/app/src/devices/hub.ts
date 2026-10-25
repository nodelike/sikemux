import { useEffect, useMemo, useSyncExternalStore } from 'react';
import { AppState } from 'react-native';
import { CoreEvent, MobileError, type ConnectionLike, type CoreListener } from '@sikemux/native';

import { problem } from '@/core/problem';
import { setAgentCatalog, type CatalogAgent } from '@shared/agents/catalog';
import { keepUnchanged, readHost, readView, type HostInfo, type Snapshot } from '@/core/protocol';
import { goOffline, thisDevice } from '@/device/identity';
import { onNetworkChange } from '@/network/connectivity';
import { dismissHostCards, reconcileCards } from '@/notify/cards';
import { shareKey, withdrawKey } from '@/notify/keys';
import { forgetDevice, pairedDevices, pairedListDamaged, readAgain, updateDevice, type PairedDevice } from './paired';
import { hostStatus, type HostStatus } from './status';

export type Live =
  /** `problem` is why the last try failed, while the phone tries again. */
  | { status: 'connecting'; problem?: string; snapshot?: Snapshot }
  | { status: 'open'; connection: ConnectionLike; snapshot?: Snapshot }
  | { status: 'closed'; problem: string; outdated?: Outdated; unpaired?: boolean; snapshot?: Snapshot };

/** Which side needs a newer Sikemux before the two can talk. */
export type Outdated = 'host' | 'phone';

/** What changed in a chat, as the host numbered it: a list of the core's `ChatChange` as JSON. */
export type ChatDelivery = { agentId: string; seq: bigint; changesJson: string };

/** A device nobody is looking at keeps its connection this long, for a quick return. */
const LINGER_MS = 30_000;
const RETRY_MS = [1000, 3000, 8000, 15_000, 30_000];
/** A connection that lasted this long dropped by chance; one that drops sooner counts as another failed try. */
const STEADY_MS = 20_000;
/** Tries a dropped connection gets before the host is shown as unreachable; until then it reads as reconnecting. */
const PATIENT_TRIES = 3;
/** A host that does not answer the unpair in this time is forgotten on the phone anyway. */
const UNPAIR_WAIT_MS = 3000;
/** Glancing at another app keeps the connections; staying away longer lets them go. */
const AWAY_MS = 10_000;
/** After the phone changes network, a connection that cannot answer in this time is replaced. */
const CHECK_MS = 5000;

const DAMAGED = "The list of paired hosts couldn't be read, so it was started again. Connect your hosts again.";

type Entry = {
  live: Live;
  watchers: number;
  /** Failed tries in a row, which sets how long the next one waits. */
  attempt: number;
  openedAt?: number;
  /** Why the last try failed, until one succeeds or the connection is let go on purpose. */
  failure?: string;
  /** A connection that was up dropped without the phone asking, and it is being made again. */
  recovering?: boolean;
  opening?: Promise<void>;
  /** The connection being made or held; a listener of any other is ignored. */
  current?: object;
  retrying?: ReturnType<typeof setTimeout>;
  lingering?: ReturnType<typeof setTimeout>;
  chats: Set<(deliveries: ChatDelivery[]) => void>;
  /** The host's agent catalog was read once, so its views can show. */
  catalogRead?: boolean;
  /** The view that arrived before the catalog, and what shows it once the catalog is in. */
  heldView?: string;
  takeView?: (viewJson: string) => void;
};

const CONNECTING: Live = { status: 'connecting' };
const FORGOTTEN: Live = { status: 'closed', problem: 'Forgotten.' };
const entries = new Map<string, Entry>();
/** Hosts left for good; a screen still open on one must not reconnect to it. */
const forgotten = new Set<string>();
const listeners = new Set<() => void>();
const hostListeners = new Map<string, Set<() => void>>();
let devices: PairedDevice[] = [];
let devicesLoaded = false;
let devicesProblem: string | null = null;
let devicesLoad = 0;
let away = false;

function changed() {
  listeners.forEach((listener) => listener());
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function hostChanged(core: string) {
  hostListeners.get(core)?.forEach((listener) => listener());
}

function subscribeHost(core: string, listener: () => void) {
  let found = hostListeners.get(core);
  if (!found) {
    found = new Set();
    hostListeners.set(core, found);
  }
  found.add(listener);
  return () => {
    found.delete(listener);
    if (!found.size && hostListeners.get(core) === found) hostListeners.delete(core);
  };
}

/** A forgotten host gets an entry that is never kept, so nothing brings it back. */
function entry(core: string): Entry {
  let found = entries.get(core);
  if (!found) {
    found = {
      live: forgotten.has(core) ? FORGOTTEN : CONNECTING,
      watchers: 0,
      attempt: 0,
      chats: new Set(),
    };
    if (!forgotten.has(core)) entries.set(core, found);
  }
  return found;
}

function set(core: string, found: Entry, live: Live) {
  found.live = live;
  if (entries.get(core) === found) hostChanged(core);
}

function outdated(error: unknown): Outdated | undefined {
  if (!MobileError.Outdated.instanceOf(error)) return undefined;
  return error.inner.macIsOlder ? 'host' : 'phone';
}

/** A host that turned this phone away, or speaks another version, stays so until someone acts. */
function settled(found: Entry): boolean {
  return found.live.status === 'closed' && (!!found.live.unpaired || !!found.live.outdated);
}

export async function reloadDevices() {
  const load = (devicesLoad += 1);
  try {
    const list = await pairedDevices();
    if (load !== devicesLoad) return;
    devices = list;
    devicesProblem = pairedListDamaged() ? DAMAGED : null;
  } catch (error) {
    if (load !== devicesLoad) return;
    devicesProblem = problem(error);
  }
  devicesLoaded = true;
  changed();
}

/** Reads the list of paired hosts from disk again, clearing the problem it showed if it now reads cleanly. */
export async function readDevicesAgain() {
  await readAgain();
  await reloadDevices();
}

function stopTimers(found: Entry) {
  clearTimeout(found.retrying);
  clearTimeout(found.lingering);
}

function scheduleRetry(core: string, found: Entry) {
  if (found.watchers === 0 || away) return;
  const wait = RETRY_MS[Math.min(found.attempt, RETRY_MS.length - 1)];
  found.attempt += 1;
  // Spread out so several hosts, or a host and its relay, are not all asked at once.
  found.retrying = setTimeout(() => open(core), wait * (0.8 + Math.random() * 0.4));
}

function release(core: string, found: Entry) {
  stopTimers(found);
  found.current = undefined;
  if (found.live.status === 'open') {
    found.live.connection.close();
    // "Seen 5m ago" means when the phone last had the host, which is when a live connection ends.
    if (!forgotten.has(core)) updateDevice(core, { lastSeen: Date.now() }).then(reloadDevices, () => {});
  }
  if (found.openedAt !== undefined && Date.now() - found.openedAt >= STEADY_MS) found.attempt = 0;
  found.openedAt = undefined;
}

function drop(core: string, reason: string, error?: unknown) {
  const found = entry(core);
  release(core, found);
  const unpaired = MobileError.Unpaired.instanceOf(error);
  const outdatedBy = outdated(error);
  found.failure = reason;
  const holding = !!found.recovering && !unpaired && !outdatedBy && found.attempt < PATIENT_TRIES && found.watchers > 0 && !away;
  if (!holding) found.recovering = false;
  set(
    core,
    found,
    holding
      ? { status: 'connecting', snapshot: found.live.snapshot }
      : { status: 'closed', problem: reason, outdated: outdatedBy, unpaired, snapshot: found.live.snapshot },
  );
  if (!settled(found)) scheduleRetry(core, found);
}

function listener(core: string, found: Entry, attempt: object): CoreListener {
  const current = () => found.current === attempt && entries.get(core) === found;
  let fresh = true;
  const take = (viewJson: string) => {
    const view = keepUnchanged(found.live.snapshot, readView(viewJson));
    // The host sends its view as soon as it lets the phone in, which can be before `connect` answers.
    set(core, found, { ...found.live, snapshot: view });
    reconcileCards(core, view, fresh);
    fresh = false;
  };
  found.takeView = (viewJson) => {
    if (current()) take(viewJson);
  };
  return {
    events(events) {
      if (!current()) return;
      const chats: ChatDelivery[] = [];
      let viewJson: string | undefined;
      for (const event of events) {
        if (CoreEvent.Chat.instanceOf(event)) chats.push(event.inner);
        // Each view is whole, so of several in one batch only the last matters.
        else if (CoreEvent.View.instanceOf(event)) viewJson = event.inner.viewJson;
      }
      if (chats.length) {
        found.chats.forEach((listen) => {
          try {
            listen(chats);
          } catch (error) {
            console.warn('sikemux: a chat could not take its events', error);
          }
        });
      }
      if (viewJson === undefined) return;
      if (found.catalogRead) take(viewJson);
      else found.heldView = viewJson;
    },
    closed() {
      if (!current() || found.live.status !== 'open') return;
      found.recovering = true;
      drop(core, 'The connection closed.');
    },
  };
}

function open(core: string): Promise<void> {
  const found = entry(core);
  if (forgotten.has(core) || away || found.live.status === 'open') return Promise.resolve();
  found.opening ??= connect(core, found).finally(() => {
    found.opening = undefined;
  });
  return found.opening;
}

async function connect(core: string, found: Entry) {
  clearTimeout(found.retrying);
  set(core, found, { status: 'connecting', problem: found.recovering ? undefined : found.failure, snapshot: found.live.snapshot });
  const attempt = {};
  found.current = attempt;
  let connection: ConnectionLike;
  try {
    const device = await thisDevice();
    connection = await device.connect(core, listener(core, found, attempt));
  } catch (error) {
    if (found.current === attempt) drop(core, problem(error), error);
    return;
  }
  if (found.current !== attempt || found.watchers === 0 || forgotten.has(core) || away) {
    connection.close();
    if (found.current === attempt) {
      found.current = undefined;
      set(core, found, { status: 'closed', problem: 'Not in use.', snapshot: found.live.snapshot });
    }
    return;
  }
  found.openedAt = Date.now();
  found.failure = undefined;
  found.recovering = false;
  set(core, found, { status: 'open', connection, snapshot: found.live.snapshot });
  if (AppState.currentState !== 'active') connection.setForeground(false).catch(() => {});
  shareKey(core, connection).then(() => withdrawIfForgotten(core));
  readCatalog(found, connection);
  hostOf(connection)
    .then((host) => {
      const known = devices.find((device) => device.core === core);
      if (known?.name === host.name && known.model === host.model && known.channel === host.channel) return;
      return updateDevice(core, { name: host.name, model: host.model, channel: host.channel }).then(reloadDevices);
    })
    .catch(() => {});
}

/** Takes the agents the host can run, which every screen names and draws agents from, then shows the view that waited on them. */
function readCatalog(found: Entry, connection: ConnectionLike) {
  connection
    .call('agentCatalog', '{}')
    .then((json) => setAgentCatalog(JSON.parse(json) as CatalogAgent[]))
    .catch(() => {})
    .finally(() => {
      found.catalogRead = true;
      const held = found.heldView;
      found.heldView = undefined;
      if (held !== undefined) found.takeView?.(held);
    });
}

const hosts = new WeakMap<ConnectionLike, Promise<HostInfo>>();

/** What the host behind `connection` says it is, asked once per connection. */
export function hostOf(connection: ConnectionLike): Promise<HostInfo> {
  let host = hosts.get(connection);
  if (!host) {
    host = connection.host().then(readHost);
    host.catch(() => hosts.delete(connection));
    hosts.set(connection, host);
  }
  return host;
}

function close(core: string, found: Entry, reason: string) {
  release(core, found);
  found.failure = undefined;
  found.recovering = false;
  set(core, found, { status: 'closed', problem: reason, snapshot: found.live.snapshot });
}

/** Drops a connection that may be dead and makes a new one now, as on a fresh start. */
function reconnect(core: string, found: Entry, reason: string) {
  close(core, found, reason);
  found.attempt = 0;
  found.recovering = true;
  open(core);
}

/** Keeps a device connected until the returned function is called. */
export function watch(core: string) {
  const found = entry(core);
  found.watchers += 1;
  clearTimeout(found.lingering);
  if (found.live.status !== 'open' && !settled(found)) open(core);
  return () => {
    found.watchers -= 1;
    if (found.watchers > 0) return;
    found.lingering = setTimeout(() => {
      if (found.watchers === 0) close(core, found, 'Not in use.');
    }, LINGER_MS);
  };
}

/** A host forgotten while it was taking the notification key gets it withdrawn again. */
function withdrawIfForgotten(core: string) {
  if (forgotten.has(core)) withdrawKey(core, undefined);
}

/** The hosts this phone holds a connection to now. */
export function openConnections(): [string, ConnectionLike][] {
  const open: [string, ConnectionLike][] = [];
  entries.forEach((found, core) => {
    if (found.live.status === 'open') open.push([core, found.live.connection]);
  });
  return open;
}

/** Gives every connected host the notification key again, as when notifications were just turned on. */
export function shareKeys() {
  openConnections().forEach(([core, connection]) => shareKey(core, connection).then(() => withdrawIfForgotten(core)));
}

function tellForeground(foreground: boolean) {
  openConnections().forEach(([, connection]) => connection.setForeground(foreground).catch(() => {}));
}

let leftAt = 0;
let leaving: ReturnType<typeof setTimeout> | undefined;
/** Set when the app goes to the background, so only coming back from there counts as a return. */
let backgrounded = false;

AppState.addEventListener('change', (state) => {
  if (state === 'background') {
    tellForeground(false);
    backgrounded = true;
    leftAt = Date.now();
    clearTimeout(leaving);
    leaving = setTimeout(() => {
      away = true;
      entries.forEach((found, core) => {
        const wasOpen = found.live.status === 'open';
        close(core, found, 'Paused while the app is away.');
        found.recovering = wasOpen;
      });
      goOffline();
    }, AWAY_MS);
    return;
  }
  // Notification Center, Control Center and Face ID only make the app inactive for a moment.
  if (state !== 'active' || !backgrounded) return;
  backgrounded = false;
  clearTimeout(leaving);
  tellForeground(true);
  // Timers do not run while the phone has the app suspended, so the time away is measured too.
  const wasAway = away || Date.now() - leftAt > AWAY_MS;
  away = false;
  entries.forEach((found, core) => {
    // A connection the phone held while it slept may be dead without having noticed yet.
    if (found.live.status === 'open' && (wasAway || !found.live.connection.isOpen())) {
      close(core, found, 'Reconnecting.');
      found.recovering = true;
    }
    if (found.watchers === 0 || found.live.status === 'open') return;
    if (found.live.status === 'closed' && found.live.unpaired) return;
    found.attempt = 0;
    open(core);
  });
});

/** Asks a connection to answer after the network changed under it, and replaces it when it cannot. */
function check(core: string, found: Entry, connection: ConnectionLike) {
  const stillThis = () => found.live.status === 'open' && found.live.connection === connection;
  if (!connection.isOpen()) {
    reconnect(core, found, 'The network changed.');
    return;
  }
  const abort = new AbortController();
  let timer: ReturnType<typeof setTimeout> | undefined;
  const late = new Promise<never>((_, fail) => {
    timer = setTimeout(() => {
      abort.abort();
      fail(new Error('No answer.'));
    }, CHECK_MS);
  });
  Promise.race([connection.host({ signal: abort.signal }), late])
    .catch(() => {
      if (stillThis()) reconnect(core, found, 'The network changed.');
    })
    .finally(() => clearTimeout(timer));
}

onNetworkChange(() => {
  if (away) return;
  entries.forEach((found, core) => {
    if (found.live.status === 'open') {
      check(core, found, found.live.connection);
      return;
    }
    if (found.watchers === 0 || settled(found)) return;
    found.attempt = 0;
    open(core);
  });
});

export function useDevices(): { devices: PairedDevice[]; loaded: boolean; problem: string | null } {
  useEffect(() => {
    if (!devicesLoaded) reloadDevices();
  }, []);
  const list = useSyncExternalStore(subscribe, () => devices);
  const loaded = useSyncExternalStore(subscribe, () => devicesLoaded);
  const unreadable = useSyncExternalStore(subscribe, () => devicesProblem);
  return { devices: list, loaded, problem: unreadable };
}

export function liveOf(core: string): Live {
  return entries.get(core)?.live ?? (forgotten.has(core) ? FORGOTTEN : CONNECTING);
}

function useHost<T>(core: string, read: () => T): T {
  return useSyncExternalStore((listener) => subscribeHost(core, listener), read);
}

/** The host's connection while it is open. */
export function connectionOf(live: Live | undefined): ConnectionLike | undefined {
  return live?.status === 'open' ? live.connection : undefined;
}

/**
 * One device's live state; the device stays connected while this is mounted and `watching`. Only that host's
 * changes re-render.
 */
export function useLive(core: string, watching = true): Live {
  useEffect(() => (watching ? watch(core) : undefined), [core, watching]);
  return useHost(core, () => liveOf(core));
}

/** How a host is doing, worded as the screens show it; the device stays connected while this is mounted and `watching`. */
export function useHostStatus(core: string, watching = true): HostStatus {
  const live = useLive(core, watching);
  const device = useSyncExternalStore(subscribe, () => devices.find((known) => known.core === core));
  return useMemo(() => hostStatus(live, device), [live, device]);
}

/**
 * Leaves a host for good: tells it to stop notifying this phone and to unpair it while it can, then drops the
 * connection, the cards about it and the host itself.
 */
export async function forget(core: string) {
  forgotten.add(core);
  const found = entries.get(core);
  const connection = connectionOf(found?.live);
  const goodbye = (async () => {
    await withdrawKey(core, connection).catch(() => {});
    await connection?.unpair();
  })().catch(() => {});
  await Promise.race([goodbye, new Promise((settle) => setTimeout(settle, UNPAIR_WAIT_MS))]);
  dismissHostCards(core);
  if (found) {
    release(core, found);
    found.live = FORGOTTEN;
  }
  entries.delete(core);
  hostChanged(core);
  await forgetDevice(core);
  await reloadDevices();
}

/** A host this phone just joined again: forgotten no longer, and worth trying however it last turned the phone away. */
export function rejoined(core: string) {
  forgotten.delete(core);
  const found = entry(core);
  if (found.live.status === 'open') return;
  found.attempt = 0;
  if (found.watchers > 0) open(core);
  else set(core, found, { status: 'connecting' });
}

export function retry(core: string) {
  const found = entry(core);
  clearTimeout(found.retrying);
  found.attempt = 0;
  open(core);
}

/** A chat's events from this host, in the batches they arrived in. Events from a connection already replaced never arrive. */
export function onChatEvents(core: string, listen: (deliveries: ChatDelivery[]) => void) {
  const found = entry(core);
  found.chats.add(listen);
  return () => {
    found.chats.delete(listen);
  };
}
