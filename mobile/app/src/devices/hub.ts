import { useEffect, useMemo, useSyncExternalStore } from 'react';
import { AppState } from 'react-native';
import { CoreEvent, MobileError, type ConnectionLike, type CoreListener } from '@sikemux/native';

import { callHost } from '@/core/hostCalls';
import { problem } from '@/core/problem';
import { translate, useT } from '@/i18n';
import { forgetAgentCatalog, setAgentCatalog } from '@shared/agents/catalog';
import { externalValue, useExternalValue } from '@shared/lib/externalValue';
import type { HostInfo } from '@shared/wire.generated';
import { keepUnchanged, readHost, readView, type Snapshot } from '@/core/protocol';
import { goOffline, notifyNetworkChange, refreshDevice, thisDevice } from '@/phone/identity';
import { onNetworkChange } from '@/network/connectivity';
import { dismissHostCards, reconcileCards } from '@/notify/cards';
import { shareKey, withdrawKey } from '@/notify/keys';
import { flushKeptViews, forgetKeptView, loadKeptView, saveKeptView } from './keptView';
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
const RETRY_MS = [250, 750, 1500, 3000, 6000, 12_000];
/** A connection that lasted this long dropped by chance; one that drops sooner counts as another failed try. */
const STEADY_MS = 20_000;
/** How long a host reads as connecting or reconnecting before showing as unreachable. */
const REACH_GRACE_MS = 15_000;
/** A host that does not answer the unpair in this time is forgotten on the phone anyway. */
const UNPAIR_WAIT_MS = 3000;
/** Glancing at another app keeps the connections; staying away longer lets them go. */
const AWAY_MS = 10_000;
/** Back in front or after a network change, a connection that cannot answer in this time is replaced. */
const PROBE_MS = 1000;
/** A try that has not ended by now is given up and tried again; sits above the core's 15s dial. */
const ATTEMPT_MS = 22_000;

type Entry = {
  live: Live;
  watchers: number;
  /** Failed tries in a row, which sets how long the next one waits. */
  attempt: number;
  openedAt?: number;
  /** Last time the host was confirmed alive on the open connection. */
  lastActivityAt?: number;
  /** When the current stretch of trying to reach this host began. */
  tryingSince?: number;
  /** True while a liveness probe is in flight on this host's open connection. */
  probing?: boolean;
  /** Why the last try failed, until one succeeds or the connection is let go on purpose. */
  failure?: string;
  /** The connection being made or held; a listener of any other is ignored. */
  current?: object;
  retrying?: ReturnType<typeof setTimeout>;
  lingering?: ReturnType<typeof setTimeout>;
  chats: Set<(deliveries: ChatDelivery[]) => void>;
};

const CONNECTING: Live = { status: 'connecting' };
const FORGOTTEN: Live = { status: 'closed', problem: 'Forgotten.' };
const entries = new Map<string, Entry>();
/** Hosts left for good; a screen still open on one must not reconnect to it. */
const forgotten = new Set<string>();
const hostListeners = new Map<string, Set<() => void>>();
/** The paired hosts as last read from disk; `problem` while the list could not be read. */
const paired = externalValue<{ devices: PairedDevice[]; loaded: boolean; problem: string | null }>({
  devices: [],
  loaded: false,
  problem: null,
});
let devicesLoad = 0;
let away = false;

const anyListeners = new Set<() => void>();

function hostChanged(core: string) {
  hostListeners.get(core)?.forEach((listener) => listener());
  anyListeners.forEach((listener) => listener());
}

/** Hears every change to the paired hosts or any host's live state. */
export function onAnyHost(listener: () => void) {
  anyListeners.add(listener);
  const offPaired = paired.subscribe(listener);
  return () => {
    anyListeners.delete(listener);
    offPaired();
  };
}

/** The paired hosts as last read, without reading them again. */
export function knownDevices(): readonly PairedDevice[] {
  return paired.get().devices;
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
    const snapshot = forgotten.has(core) ? undefined : loadKeptView(core);
    found = {
      live: forgotten.has(core) ? FORGOTTEN : snapshot ? { status: 'connecting', snapshot } : CONNECTING,
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
    const devices = await pairedDevices();
    if (load !== devicesLoad) return;
    paired.set({
      devices,
      loaded: true,
      problem: pairedListDamaged()
        ? translate("The list of paired hosts couldn't be read, so it was started again. Connect your hosts again.")
        : null,
    });
  } catch (error) {
    if (load !== devicesLoad) return;
    paired.set({ ...paired.get(), loaded: true, problem: problem(error) });
  }
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
  if (found.watchers === 0 || away || backgrounded) return;
  const wait = RETRY_MS[Math.min(found.attempt, RETRY_MS.length - 1)];
  found.attempt += 1;
  // Spread out so several hosts, or a host and its relay, are not all asked at once.
  found.retrying = setTimeout(() => open(core), wait * (0.8 + Math.random() * 0.4));
}

function inGrace(found: Entry): boolean {
  return found.tryingSince !== undefined && Date.now() - found.tryingSince < REACH_GRACE_MS;
}

/** The shared endpoint may only be torn down when no host on it is live or waiting on its own probe. */
function canRefreshEndpoint(): boolean {
  for (const found of entries.values()) {
    if (found.live.status === 'open' || found.probing) return false;
  }
  return true;
}

function release(core: string, found: Entry) {
  stopTimers(found);
  found.current = undefined;
  found.probing = false;
  if (found.live.status === 'open') {
    found.live.connection.close();
    // "Seen 5m ago" means when the host was last heard alive, never when a frozen socket was torn down.
    const seen = found.lastActivityAt ?? found.openedAt;
    if (!forgotten.has(core) && seen !== undefined) updateDevice(core, { lastSeen: seen }).then(reloadDevices, () => {});
  }
  if (found.openedAt !== undefined && Date.now() - found.openedAt >= STEADY_MS) found.attempt = 0;
  found.openedAt = undefined;
  found.lastActivityAt = undefined;
}

function drop(core: string, reason: string, error?: unknown) {
  const found = entry(core);
  const wasOpen = found.live.status === 'open';
  release(core, found);
  if (wasOpen) found.tryingSince = Date.now();
  const unpaired = MobileError.Unpaired.instanceOf(error);
  const outdatedBy = outdated(error);
  if (unpaired) forgetKeptView(core);
  const snapshot = unpaired ? undefined : found.live.snapshot;
  found.failure = reason;
  const holding = !unpaired && !outdatedBy && found.watchers > 0 && !away && !backgrounded && inGrace(found);
  set(
    core,
    found,
    holding ? { status: 'connecting', snapshot } : { status: 'closed', problem: reason, outdated: outdatedBy, unpaired, snapshot },
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
    saveKeptView(core, view);
    reconcileCards(core, view, fresh);
    fresh = false;
  };
  return {
    events(events) {
      if (!current()) return;
      found.lastActivityAt = Date.now();
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
      if (viewJson !== undefined) take(viewJson);
    },
    closed() {
      if (!current() || found.live.status !== 'open') return;
      console.info(`sikemux: host ${core.slice(0, 8)} connection closed`);
      drop(core, translate('The connection closed.'));
    },
  };
}

/** Starts a try unless one is under way; `again` gives up the one under way first. */
function open(core: string, again = false) {
  const found = entry(core);
  if (forgotten.has(core) || away || backgrounded || found.live.status === 'open') return;
  if (found.current && !again) return;
  if (found.tryingSince === undefined) found.tryingSince = Date.now();
  connect(core, found);
}

async function connect(core: string, found: Entry) {
  clearTimeout(found.retrying);
  set(core, found, { status: 'connecting', problem: inGrace(found) ? undefined : found.failure, snapshot: found.live.snapshot });
  const attempt = {};
  found.current = attempt;
  const late = setTimeout(() => {
    if (found.current !== attempt) return;
    console.info(`sikemux: host ${core.slice(0, 8)} dial stalled after ${ATTEMPT_MS}ms`);
    drop(core, translate('The host did not answer in time.'));
    if (canRefreshEndpoint()) void refreshDevice();
  }, ATTEMPT_MS);
  let connection: ConnectionLike;
  try {
    const device = await thisDevice();
    connection = await device.connect(core, listener(core, found, attempt));
  } catch (error) {
    if (found.current === attempt) drop(core, problem(error), error);
    return;
  } finally {
    clearTimeout(late);
  }
  if (found.current !== attempt || found.watchers === 0 || forgotten.has(core) || away || backgrounded) {
    connection.close();
    if (found.current === attempt) {
      found.current = undefined;
      set(core, found, { status: 'closed', problem: translate('Not in use.'), snapshot: found.live.snapshot });
    }
    return;
  }
  const now = Date.now();
  found.openedAt = now;
  found.lastActivityAt = now;
  found.tryingSince = undefined;
  found.failure = undefined;
  set(core, found, { status: 'open', connection, snapshot: found.live.snapshot });
  if (AppState.currentState !== 'active') connection.setForeground(false).catch(() => {});
  shareKey(core, connection).then(() => withdrawIfForgotten(core));
  readCatalog(core, connection);
  hostOf(connection)
    .then((host) => {
      const known = knownDevices().find((device) => device.core === core);
      const access = host.access ?? known?.access;
      if (known?.model === host.model && known.channel === host.channel && known.access === access) return;
      return updateDevice(core, { model: host.model, channel: host.channel, ...(access ? { access } : {}) }).then(reloadDevices);
    })
    .catch((error: unknown) => console.warn('sikemux: could not keep what the host says it is', error));
}

/** Takes the agents the host can run, which every screen names and draws agents from. */
function readCatalog(core: string, connection: ConnectionLike) {
  callHost(connection, 'agentCatalog', {})
    .then((catalog) => setAgentCatalog(core, catalog))
    .catch((error: unknown) => console.warn("sikemux: could not read the host's agents", error));
}

const hosts = new WeakMap<ConnectionLike, Promise<HostInfo>>();

/** What the host behind `connection` says it is, asked once per connection. */
function hostOf(connection: ConnectionLike): Promise<HostInfo> {
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
  found.tryingSince = undefined;
  set(core, found, { status: 'closed', problem: reason, snapshot: found.live.snapshot });
}

/** Drops a connection that may be dead and makes a new one now, as on a fresh start. */
function reconnect(core: string, found: Entry, reason: string) {
  close(core, found, reason);
  if (canRefreshEndpoint()) void refreshDevice();
  found.attempt = 0;
  found.tryingSince = Date.now();
  open(core, true);
}

/** Keeps a device connected until the returned function is called. */
export function watch(core: string) {
  const found = entry(core);
  found.watchers += 1;
  clearTimeout(found.lingering);
  if (found.live.status !== 'open' && !settled(found)) {
    if (found.tryingSince === undefined) found.tryingSince = Date.now();
    open(core);
  }
  return () => {
    found.watchers -= 1;
    if (found.watchers > 0) return;
    found.lingering = setTimeout(() => {
      if (found.watchers === 0) close(core, found, translate('Not in use.'));
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
    flushKeptViews();
    backgrounded = true;
    leftAt = Date.now();
    entries.forEach((found) => clearTimeout(found.retrying));
    clearTimeout(leaving);
    leaving = setTimeout(() => {
      away = true;
      entries.forEach((found, core) => {
        close(core, found, translate('Paused while the app is away.'));
      });
      void goOffline();
    }, AWAY_MS);
    return;
  }
  // Notification Center, Control Center and Face ID only make the app inactive for a moment.
  if (state !== 'active' || !backgrounded) return;
  backgrounded = false;
  clearTimeout(leaving);
  const elapsed = Date.now() - leftAt;
  // Timers do not run while the phone has the app suspended, so the time away is measured too.
  const wasAway = away || elapsed > AWAY_MS;
  away = false;
  console.info(`sikemux: resume after ${elapsed}ms (away=${wasAway})`);
  if (wasAway) {
    entries.forEach((found, core) => {
      if (found.live.status === 'open') close(core, found, translate('Reconnecting.'));
    });
    void refreshDevice();
    entries.forEach((found, core) => {
      if (found.watchers === 0) return;
      if (found.live.status === 'closed' && found.live.unpaired) return;
      found.attempt = 0;
      found.tryingSince = Date.now();
      open(core, true);
    });
    return;
  }
  tellForeground(true);
  void notifyNetworkChange();
  const toOpen: [string, Entry][] = [];
  entries.forEach((found, core) => {
    if (found.live.status === 'open') {
      check(core, found, found.live.connection, PROBE_MS);
      return;
    }
    if (found.watchers === 0) return;
    if (found.live.status === 'closed' && found.live.unpaired) return;
    toOpen.push([core, found]);
  });
  if (toOpen.some(([, found]) => found.current) && canRefreshEndpoint()) void refreshDevice();
  toOpen.forEach(([core, found]) => {
    found.attempt = 0;
    found.tryingSince = Date.now();
    open(core, true);
  });
});

/** Asks a connection to answer after resume or network change, and replaces it when it cannot. */
function check(core: string, found: Entry, connection: ConnectionLike, wait = PROBE_MS) {
  const stillThis = () => found.live.status === 'open' && found.live.connection === connection;
  if (!connection.isOpen()) {
    reconnect(core, found, translate('The network changed.'));
    return;
  }
  found.probing = true;
  const startedAt = Date.now();
  const abort = new AbortController();
  let timer: ReturnType<typeof setTimeout> | undefined;
  const late = new Promise<never>((_, fail) => {
    timer = setTimeout(() => {
      abort.abort();
      fail(new Error(translate('No answer.')));
    }, wait);
  });
  Promise.race([connection.host({ signal: abort.signal }), late])
    .then(
      () => {
        if (stillThis()) {
          found.probing = false;
          found.lastActivityAt = Date.now();
        }
      },
      () => {
        if (!stillThis()) return;
        found.probing = false;
        // Live view or chat traffic arrived while the probe was out, so the connection is healthy.
        if (found.lastActivityAt !== undefined && found.lastActivityAt >= startedAt) return;
        console.info(`sikemux: host ${core.slice(0, 8)} probe failed within ${wait}ms`);
        reconnect(core, found, translate('The network changed.'));
      },
    )
    .finally(() => clearTimeout(timer));
}

onNetworkChange((change) => {
  if (away || backgrounded) return;
  console.info(`sikemux: network ${change}`);
  void notifyNetworkChange();
  const toOpen: [string, Entry][] = [];
  entries.forEach((found, core) => {
    if (found.live.status === 'open') {
      check(core, found, found.live.connection, PROBE_MS);
      return;
    }
    if (found.watchers === 0 || settled(found)) return;
    toOpen.push([core, found]);
  });
  if (toOpen.length > 0 && canRefreshEndpoint()) void refreshDevice();
  toOpen.forEach(([core, found]) => {
    found.attempt = 0;
    open(core, true);
  });
});

export function useDevices(): { devices: PairedDevice[]; loaded: boolean; problem: string | null } {
  useEffect(() => {
    if (!paired.get().loaded) reloadDevices();
  }, []);
  return useExternalValue(paired);
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

/** A host's live state only while `needed`, for a screen already watching it; until then its changes re-render nothing. */
export function useLiveWhile(core: string, needed: boolean): Live | undefined {
  return useHost(core, () => (needed ? liveOf(core) : undefined));
}

/** How a host is doing, worded as the screens show it; the device stays connected while this is mounted and `watching`. */
export function useHostStatus(core: string, watching = true): HostStatus {
  const t = useT();
  const live = useLive(core, watching);
  const device = useExternalValue(paired, ({ devices }) => devices.find((known) => known.core === core));
  return useMemo(() => hostStatus(live, device, undefined, t), [live, device, t]);
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
  forgetAgentCatalog(core);
  forgetKeptView(core);
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
  found.tryingSince = Date.now();
  if (found.watchers > 0) open(core);
  else set(core, found, { status: 'connecting', snapshot: found.live.snapshot });
}

/** Gives up any try under way and starts a fresh one now. */
export function retry(core: string) {
  const found = entry(core);
  clearTimeout(found.retrying);
  found.attempt = 0;
  found.tryingSince = Date.now();
  if (canRefreshEndpoint()) void refreshDevice();
  open(core, true);
}

/** A chat's events from this host, in the batches they arrived in. Events from a connection already replaced never arrive. */
export function onChatEvents(core: string, listen: (deliveries: ChatDelivery[]) => void) {
  const found = entry(core);
  found.chats.add(listen);
  return () => {
    found.chats.delete(listen);
  };
}
