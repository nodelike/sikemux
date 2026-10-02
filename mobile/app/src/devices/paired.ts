import { File, Paths } from 'expo-file-system';

import type { BuildChannel } from '@/core/protocol';
import type { DeviceKind } from '@/ui/Icon';

export type Access = 'full' | 'watch';

/** A computer this phone paired with, known by its core's key. */
export type PairedDevice = {
  core: string;
  access: Access;
  pairedAt: number;
  name?: string;
  model?: string;
  channel?: BuildChannel;
  /** The project the device screen is scoped to; absent shows them all. */
  project?: string;
  /** The Mac's theme colours as last seen, so its screens open in them before it connects. */
  palette?: Record<string, string>;
  /** The Mac's pane backdrop as last seen, its picture saved on the phone. */
  backdrop?: { texture: boolean; image?: { id: string; uri: string } };
  lastSeen?: number;
};

/** Nothing here is secret (a Mac's key is public), so it lives in a file rather than the Keychain. */
const store = new File(Paths.document, 'paired-devices.json');

let cached: PairedDevice[] | undefined;
let writing: Promise<unknown> = Promise.resolve();

async function load(): Promise<PairedDevice[]> {
  cached ??= store.exists ? (JSON.parse(await store.text()) as PairedDevice[]) : [];
  return cached;
}

export async function pairedDevices(): Promise<PairedDevice[]> {
  await writing;
  return load();
}

/** Changes run one after another on the latest list, so two at once never undo each other. */
function change(edit: (devices: PairedDevice[]) => PairedDevice[]): Promise<void> {
  const next = writing.then(async () => {
    const devices = edit(await load());
    store.write(JSON.stringify(devices));
    cached = devices;
  });
  writing = next.catch(() => {});
  return next;
}

export function rememberDevice(device: PairedDevice): Promise<void> {
  return change((devices) => [...devices.filter((known) => known.core !== device.core), device]);
}

/** Changes a Mac still paired; one forgotten in the meantime stays forgotten. */
export function updateDevice(core: string, update: Partial<PairedDevice>): Promise<void> {
  return change((devices) => devices.map((device) => (device.core === core ? { ...device, ...update } : device)));
}

export function forgetDevice(core: string): Promise<void> {
  return change((devices) => devices.filter((known) => known.core !== core));
}

export function shortKey(key: string): string {
  return key.slice(0, 8);
}

export function deviceName(device: Pick<PairedDevice, 'name' | 'core'>): string {
  return device.name ?? `Mac ${shortKey(device.core)}`;
}

/** Builds other than the stable release are named, since one Mac pairs once per channel. */
export function channelLabel(channel: BuildChannel | undefined): string | null {
  if (channel === 'dev') return 'Dev';
  if (channel === 'nightly') return 'Nightly';
  return null;
}

export function deviceKind(model: string | undefined): DeviceKind {
  if (!model) return 'laptop';
  if (/macbook/i.test(model)) return 'laptop';
  if (/mini/i.test(model)) return 'mini';
  return 'desktop';
}
