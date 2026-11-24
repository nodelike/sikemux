import { useEffect, useState } from 'react';
import * as SecureStore from 'expo-secure-store';
import { Device, DeviceIdentity, newDeviceKey, type DeviceLike } from '@sikemux/native';

import { translate } from '@/i18n';
import { askForRelaysAgain, currentRelays, relaySettings, updateRequired } from '@/network/network';

const KEY_ITEM = 'sikemux.device-key';

function hex(bytes: ArrayBuffer): string {
  return Array.from(new Uint8Array(bytes), (byte) => byte.toString(16).padStart(2, '0')).join('');
}

function bytes(text: string): ArrayBuffer {
  const out = new Uint8Array(text.length / 2);
  for (let i = 0; i < out.length; i += 1) out[i] = parseInt(text.slice(i * 2, i * 2 + 2), 16);
  return out.buffer;
}

/** This phone's key, made once and kept in the Keystore. */
async function deviceKey(): Promise<ArrayBuffer> {
  const stored = await SecureStore.getItemAsync(KEY_ITEM);
  if (stored) {
    if (!/^[0-9a-f]{64}$/.test(stored)) throw new Error(translate("This phone's key is damaged; reinstall the app to connect again."));
    return bytes(stored);
  }
  const key = newDeviceKey();
  await SecureStore.setItemAsync(KEY_ITEM, hex(key));
  return key;
}

type NativeDevice = DeviceLike & { networkChanged?: () => Promise<void>; refresh?: () => Promise<void> };

let identity: Promise<DeviceIdentity> | undefined;
let online: Promise<DeviceLike> | undefined;
let ready: NativeDevice | undefined;
let closing: Promise<void> = Promise.resolve();
let generation = 0;
let joining = 0;

/** This phone's key, for proving who it is to the accounts server; it never goes on the network. A failure is not kept. */
export async function deviceIdentity(): Promise<DeviceIdentity> {
  if (!identity) {
    const coming = deviceKey().then((key) => new DeviceIdentity(key));
    identity = coming;
    coming.catch(() => {
      if (identity === coming) identity = undefined;
    });
  }
  return identity;
}

/** Coming online takes a second; one that has not by now never will, and the next call starts again. */
const ONLINE_MS = 12_000;
/** Cap endpoint close so a wedged socket after sleep never holds up the next bind. */
const CLOSE_WAIT_MS = 200;

function after(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function inTime(creating: Promise<DeviceLike>): Promise<DeviceLike> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const late = new Promise<never>((_, fail) => {
    timer = setTimeout(() => {
      creating.then(
        (device) => device.close(),
        () => {},
      );
      fail(new Error(translate('This phone could not get online.')));
    }, ONLINE_MS);
  });
  return Promise.race([creating, late]).finally(() => clearTimeout(timer));
}

/** This phone on the network, unless the app is too old to use it. A failure is not kept, so the next call tries again. */
export function thisDevice(): Promise<DeviceLike> {
  if (!online) {
    const gen = generation;
    const coming = closing
      .then(() => Promise.all([deviceIdentity(), currentRelays()]))
      .then(async ([me, relays]) => {
        if (generation !== gen) throw new Error(translate('The network changed.'));
        if (updateRequired()) throw new Error(translate('Update Sikemux to reach your hosts.'));
        const device = await inTime(Device.create(me, relaySettings(relays)));
        if (generation !== gen) {
          void device.close().catch(() => {});
          throw new Error(translate('The network changed.'));
        }
        ready = device as NativeDevice;
        return device;
      });
    online = coming;
    coming.catch(() => {
      if (online === coming) {
        online = undefined;
        ready = undefined;
      }
    });
  }
  return online;
}

/** Keeps the phone online while a host decides whether to let it in, which can outlast the app being in front. */
export async function whileJoining<T>(work: (device: DeviceLike) => Promise<T>): Promise<T> {
  joining += 1;
  try {
    return await work(await thisDevice());
  } finally {
    joining -= 1;
  }
}

/** Takes the phone off the network while the app is away; the next call to `thisDevice` waits for close and brings it back. */
export function goOffline(): Promise<void> {
  if (joining > 0) return closing;
  generation += 1;
  const going = online;
  online = undefined;
  ready = undefined;
  if (!going) return closing;
  const stop = going
    .catch(() => undefined)
    .then((device) => (device ? Promise.race([device.close().catch(() => {}), after(CLOSE_WAIT_MS)]) : undefined));
  closing = closing.then(() => stop);
  return closing;
}

/** Tells a live native endpoint that the OS network changed, when the native build supports it. */
export function notifyNetworkChange(): Promise<void> {
  askForRelaysAgain();
  return ready?.networkChanged ? ready.networkChanged().catch(() => {}) : Promise.resolve();
}

/** Refreshes the underlying endpoint in place when supported, otherwise tears it down cleanly before the next bind. */
export function refreshDevice(): Promise<void> {
  askForRelaysAgain();
  if (joining > 0) return closing;
  const live = ready;
  if (live?.refresh) {
    const refreshing = live.refresh().catch(() => {});
    closing = closing.then(() => refreshing);
    return closing;
  }
  return goOffline();
}

/**
 * Makes this phone a new device: the account may still hold the old key, and anything it signed for
 * that account must not follow the phone into another one.
 */
export async function rotateDeviceKey(): Promise<void> {
  await SecureStore.deleteItemAsync(KEY_ITEM);
  identity = undefined;
  await goOffline();
}

/** This phone's key. */
export function useDeviceId(): string | undefined {
  const [id, setId] = useState<string>();
  useEffect(() => {
    deviceIdentity()
      .then((me) => setId(me.id()))
      .catch(() => {});
  }, []);
  return id;
}
