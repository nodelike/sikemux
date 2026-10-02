import { useEffect, useState } from 'react';
import * as SecureStore from 'expo-secure-store';
import { Device, newDeviceKey, type DeviceLike } from '@sikemux/native';

const KEY_ITEM = 'sikemux.device-key';
/** The key stays on this phone: a backup restored onto another must not make it the same device. */
const KEY_OPTIONS = { keychainAccessible: SecureStore.WHEN_UNLOCKED_THIS_DEVICE_ONLY };

function hex(bytes: ArrayBuffer): string {
  return Array.from(new Uint8Array(bytes), (byte) => byte.toString(16).padStart(2, '0')).join('');
}

function bytes(text: string): ArrayBuffer {
  const out = new Uint8Array(text.length / 2);
  for (let i = 0; i < out.length; i += 1) out[i] = parseInt(text.slice(i * 2, i * 2 + 2), 16);
  return out.buffer;
}

/** This phone's key, made once and kept in the Keychain or Keystore. */
async function deviceKey(): Promise<ArrayBuffer> {
  const stored = await SecureStore.getItemAsync(KEY_ITEM, KEY_OPTIONS);
  if (stored) {
    if (!/^[0-9a-f]{64}$/.test(stored)) throw new Error("This phone's key is damaged; reinstall the app to pair again.");
    return bytes(stored);
  }
  const key = newDeviceKey();
  await SecureStore.setItemAsync(KEY_ITEM, hex(key), KEY_OPTIONS);
  return key;
}

let online: Promise<DeviceLike> | undefined;
let pairing = 0;

/** This phone on the network. A failure is not kept, so the next call tries again. */
export function thisDevice(): Promise<DeviceLike> {
  if (!online) {
    const coming = deviceKey().then((key) => Device.create(key));
    online = coming;
    coming.catch(() => {
      if (online === coming) online = undefined;
    });
  }
  return online;
}

/** Keeps the phone online while it pairs, which can outlast the app being in front. */
export async function whilePairing<T>(work: (device: DeviceLike) => Promise<T>): Promise<T> {
  pairing += 1;
  try {
    return await work(await thisDevice());
  } finally {
    pairing -= 1;
  }
}

/** Takes the phone off the network while the app is away; the next call to `thisDevice` brings it back. */
export async function goOffline() {
  const going = online;
  if (!going || pairing > 0) return;
  online = undefined;
  const device = await going.catch(() => undefined);
  await device?.close();
}

/** This phone's key, once it is online. */
export function useDeviceId(): string | undefined {
  const [id, setId] = useState<string>();
  useEffect(() => {
    thisDevice()
      .then((device) => setId(device.id()))
      .catch(() => {});
  }, []);
  return id;
}
