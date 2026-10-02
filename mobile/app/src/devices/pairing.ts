import { Platform } from 'react-native';
import { router } from 'expo-router';
import * as ExpoDevice from 'expo-device';
import * as Clipboard from 'expo-clipboard';
import { MobileError, parsePairingLink, type PairingLink } from '@sikemux/native';

import { whilePairing } from '@/device/identity';
import { rememberDevice, type Access } from './paired';
import { reloadDevices } from './hub';

export type Failure = { title: string; detail: string };

export function phoneName(): string {
  return ExpoDevice.deviceName ?? ExpoDevice.modelName ?? 'Phone';
}

export function failure(error: unknown): Failure {
  if (MobileError.WrongCode.instanceOf(error)) {
    return { title: 'That code has changed', detail: 'The Mac makes a new one every few minutes, and after a wrong one.' };
  }
  if (MobileError.Refused.instanceOf(error)) {
    return { title: 'The Mac said no', detail: error.inner.message };
  }
  if (MobileError.Unpaired.instanceOf(error)) {
    return { title: 'Pair again', detail: 'This Mac no longer knows this phone.' };
  }
  if (MobileError.Connection.instanceOf(error)) {
    return { title: "Can't reach the Mac", detail: 'Check it is awake and remote access is on in Settings → Devices.' };
  }
  return { title: 'Pairing stopped', detail: String(error) };
}

/** Pairs with the Mac in `link`, waiting while the person there decides. Aborting `signal` stops it. */
export async function pair(link: PairingLink, signal?: AbortSignal): Promise<void> {
  const access = await whilePairing((device) =>
    device.pair(link.core, link.code, phoneName(), Platform.OS, signal ? { signal } : undefined),
  );
  // React Native's AbortSignal has no throwIfAborted.
  if (signal?.aborted) throw new Error('Pairing was cancelled.');
  await rememberDevice({ core: link.core, access: access as Access, pairedAt: Date.now() });
  await reloadDevices();
}

export async function linkFromClipboard(): Promise<PairingLink | undefined> {
  const text = await Clipboard.getStringAsync();
  return parsePairingLink(text.trim());
}

export function openPairing(link: PairingLink, how: 'push' | 'replace' = 'replace') {
  router[how]({ pathname: '/pair', params: { core: link.core, code: link.code } });
}

/** Opens pairing for a link the person copied, as from the Mac's Devices page. */
export async function pasteLink(how: 'push' | 'replace' = 'push') {
  const link = await linkFromClipboard();
  if (link) openPairing(link, how);
  return link !== undefined;
}
