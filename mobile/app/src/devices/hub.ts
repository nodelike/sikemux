import { useEffect, useSyncExternalStore } from 'react';
import { AppState } from 'react-native';
import { CoreEvent, MobileError, type ConnectionLike, type CoreListener } from '@sikemux/native';

import { channelName, type Snapshot } from '@/core/protocol';
import { goOffline, thisDevice } from '@/device/identity';
import { forgetDevice, pairedDevices, updateDevice, type PairedDevice } from './paired';

export type Live =
  | { status: 'connecting'; snapshot?: Snapshot }
  | { status: 'open'; connection: ConnectionLike; snapshot?: Snapshot }
  | { status: 'closed'; problem: string; outdated?: Outdated; unpaired?: boolean; snapshot?: Snapshot };

/** Which side needs a newer Sikemux before the two can talk. */
export type Outdated = 'mac' | 'phone';

/** One of a chat's events as the Mac numbered it. */
export type ChatDelivery = { agentId: string; seq: bigint; eventJson: string };

/** A device nobody is looking at keeps its connection this long, for a quick return. */
const LINGER_MS = 30_000;
const RETRY_MS = [1000, 3000, 8000, 15_000, 30_000];
/** A Mac that does not answer the unpair in this time is forgotten on the phone anyway. */
const UNPAIR_WAIT_MS = 3000;
/** Glancing at another app keeps the connections; staying away longer lets them go. */
const AWAY_MS = 10_000;

type Entry = {
  live: Live;
  watchers: number;
  attempt: number;
  opening?: Promise<void>;
  /** The connection being made or held; a listener of any other is ignored. */
  current?: object;
  retrying?: ReturnType<typeof setTimeout>;
  lingering?: ReturnType<typeof setTimeout>;
  chats: Set<(deliveries: ChatDelivery[]) => void>;
  output: Set<(session: bigint, bytes: ArrayBuffer) => void>;
};

const CONNECTING: Live = { status: 'connecting' };
const entries = new Map<string, Entry>();
const listeners = new Set<() => void>();
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

function entry(core: string): Entry {
  let found = entries.get(core);
  if (!found) {
    found = { live: CONNECTING, watchers: 0, attempt: 0, chats: new Set(), output: new Set() };
    entries.set(core, found);
  }
  return found;
}

function set(found: Entry, live: Live) {
  found.live = live;
  changed();
}

function outdated(error: unknown): Outdated | undefined {
  if (!MobileError.Outdated.instanceOf(error)) return undefined;
  return error.inner.macIsOlder ? 'mac' : 'phone';
}

export function problem(error: unknown): string {
  if (MobileError.Refused.instanceOf(error)) return error.inner.message;
  if (MobileError.Connection.instanceOf(error)) return error.inner.message;
  if (MobileError.Unpaired.instanceOf(error)) return 'This Mac no longer knows this phone. Pair with it again.';
  return error instanceof Error ? error.message : String(error);
}

/** Macs left for good; a screen still open on one must not reconnect to it. */
const forgotten = new Set<string>();

export async function reloadDevices() {
  const load = (devicesLoad += 1);
  try {
    const list = await pairedDevices();
    if (load !== devicesLoad) return;
    devices = list;
    devicesProblem = null;
    devices.forEach((device) => forgotten.delete(device.core));
  } catch (error) {
    if (load !== devicesLoad) return;
    devicesProblem = problem(error);
  }
  devicesLoaded = true;
  changed();
}

function stopTimers(found: Entry) {
  clearTimeout(found.retrying);
  clearTimeout(found.lingering);
}

function scheduleRetry(core: string, found: Entry) {
  if (found.watchers === 0 || away) return;
  const wait = RETRY_MS[Math.min(found.attempt, RETRY_MS.length - 1)];
  found.attempt += 1;
  // Spread out so several Macs, or a Mac and its relay, are not all asked at once.
  found.retrying = setTimeout(() => open(core), wait * (0.8 + Math.random() * 0.4));
}

function drop(core: string, reason: string, error?: unknown) {
  const found = entry(core);
  stopTimers(found);
  found.current = undefined;
  if (found.live.status === 'open') found.live.connection.close();
  const unpaired = MobileError.Unpaired.instanceOf(error);
  set(found, { status: 'closed', problem: reason, outdated: outdated(error), unpaired, snapshot: found.live.snapshot });
  if (!unpaired && !outdated(error)) scheduleRetry(core, found);
}

function listener(core: string, found: Entry, attempt: object): CoreListener {
  return {
    output(session, bytes) {
      found.output.forEach((listen) => listen(session, bytes));
    },
    events(events) {
      const chats: ChatDelivery[] = [];
      for (const event of events) {
        if (CoreEvent.Chat.instanceOf(event)) chats.push(event.inner);
        // The Mac sends its view as soon as it lets the phone in, which can be before `connect` answers.
        else if (CoreEvent.View.instanceOf(event) && found.current === attempt) {
          set(found, { ...found.live, snapshot: event.inner.view });
        }
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
    },
    closed() {
      if (found.current === attempt && found.live.status === 'open') drop(core, 'The connection closed.');
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
  set(found, { status: 'connecting', snapshot: found.live.snapshot });
  const attempt = {};
  found.current = attempt;
  let connection: ConnectionLike;
  try {
    const device = await thisDevice();
    connection = await device.connect(core, listener(core, found, attempt));
  } catch (error) {
    drop(core, problem(error), error);
    return;
  }
  if (found.watchers === 0 || forgotten.has(core) || away) {
    connection.close();
    set(found, { status: 'closed', problem: 'Not in use.', snapshot: found.live.snapshot });
    return;
  }
  found.attempt = 0;
  set(found, { status: 'open', connection, snapshot: found.live.snapshot });
  connection
    .host()
    .then((host) => updateDevice(core, { name: host.name, model: host.model, channel: channelName(host.channel), lastSeen: Date.now() }))
    .then(reloadDevices)
    .catch(() => {});
}

function close(found: Entry, reason: string) {
  stopTimers(found);
  found.current = undefined;
  if (found.live.status === 'open') found.live.connection.close();
  set(found, { status: 'closed', problem: reason, snapshot: found.live.snapshot });
}

/** Keeps a device connected until the returned function is called. */
export function watch(core: string) {
  const found = entry(core);
  found.watchers += 1;
  clearTimeout(found.lingering);
  if (found.live.status !== 'open') open(core);
  return () => {
    found.watchers -= 1;
    if (found.watchers > 0) return;
    found.lingering = setTimeout(() => {
      if (found.watchers === 0) close(found, 'Not in use.');
    }, LINGER_MS);
  };
}

let leftAt = 0;
let leaving: ReturnType<typeof setTimeout> | undefined;

AppState.addEventListener('change', (state) => {
  if (state === 'background') {
    leftAt = Date.now();
    clearTimeout(leaving);
    leaving = setTimeout(() => {
      away = true;
      entries.forEach((found) => close(found, 'Paused while the app is away.'));
      goOffline();
    }, AWAY_MS);
    return;
  }
  if (state !== 'active') return;
  clearTimeout(leaving);
  const wasAway = away || Date.now() - leftAt > AWAY_MS;
  away = false;
  entries.forEach((found, core) => {
    if (found.watchers === 0) return;
    // A connection the phone held while it slept may be dead without having noticed yet.
    const stale = found.live.status === 'open' && (wasAway || !found.live.connection.isOpen());
    if (stale) close(found, 'Reconnecting.');
    if (found.live.status !== 'open') {
      found.attempt = 0;
      open(core);
    }
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
  return entries.get(core)?.live ?? CONNECTING;
}

/** One device's live state; the device stays connected while this is mounted. */
export function useLive(core: string): Live {
  useEffect(() => watch(core), [core]);
  return useSyncExternalStore(subscribe, () => liveOf(core));
}

/** Leaves a Mac for good: asks it to unpair this phone while it can, then drops the connection and forgets it. */
export async function forget(core: string) {
  forgotten.add(core);
  const found = entry(core);
  if (found.live.status === 'open') {
    const asked = found.live.connection.unpair().catch(() => {});
    await Promise.race([asked, new Promise((settle) => setTimeout(settle, UNPAIR_WAIT_MS))]);
  }
  stopTimers(found);
  if (found.live.status === 'open') found.live.connection.close();
  entries.delete(core);
  await forgetDevice(core);
  await reloadDevices();
}

export function retry(core: string) {
  const found = entry(core);
  clearTimeout(found.retrying);
  found.attempt = 0;
  open(core);
}

/** A chat's events from this Mac, in the batches they arrived in. */
export function onChatEvents(core: string, listen: (deliveries: ChatDelivery[]) => void) {
  const found = entry(core);
  found.chats.add(listen);
  return () => {
    found.chats.delete(listen);
  };
}

export function onOutput(core: string, listen: (session: bigint, bytes: ArrayBuffer) => void) {
  const found = entry(core);
  found.output.add(listen);
  return () => {
    found.output.delete(listen);
  };
}
