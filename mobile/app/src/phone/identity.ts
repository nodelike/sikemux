import { useEffect, useState } from 'react';
import * as SecureStore from 'expo-secure-store';
import { Device, DeviceIdentity, newDeviceKey, type DeviceLike } from '@sikemux/native';

import { currentRelays, relaySettings, updateRequired } from '@/network/network';

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
    if (!/^[0-9a-f]{64}$/.test(stored)) throw new Error("This phone's key is damaged; reinstall the app to connect again.");
    return bytes(stored);
  }
  const key = newDeviceKey();
  await SecureStore.setItemAsync(KEY_ITEM, hex(key));
  return key;
}

let identity: Promise<DeviceIdentity> | undefined;
let online: Promise<DeviceLike> | undefined;
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

/** This phone on the network, unless the app is too old to use it. A failure is not kept, so the next call tries again. */
export function thisDevice(): Promise<DeviceLike> {
  if (!online) {
    const coming = Promise.all([deviceIdentity(), currentRelays()]).then(([me, relays]) => {
      if (updateRequired()) throw new Error('Update Sikemux to reach your hosts.');
      return Device.create(me, relaySettings(relays));
    });
    online = coming;
    coming.catch(() => {
      if (online === coming) online = undefined;
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

/** Takes the phone off the network while the app is away; the next call to `thisDevice` brings it back. */
export async function goOffline() {
  const going = online;
  if (!going || joining > 0) return;
  online = undefined;
  const device = await going.catch(() => undefined);
  await device?.close();
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
