import * as Application from 'expo-application';
import * as Crypto from 'expo-crypto';
import * as Notifications from 'expo-notifications';
import * as SecureStore from 'expo-secure-store';
import type { ApnsEnvironment, PushApp } from '@sikemux/protocol';

import { notifier } from '../../modules/notify';
import { clearPushToken, setPushToken, type TokenSource } from '@/account/api';
import { deviceIdentity } from '@/phone/identity';
import { notificationsChoice } from './setting';

/** What the server last took from this phone, so an unchanged token is not sent again. */
type SentToken = { device: string; tokenSha256: string; app: PushApp; apnsEnvironment?: ApnsEnvironment; at: number };

const ITEM = 'sikemux.push-token';
/** FCM tokens can go quiet without changing; sending one again now and then keeps the server's copy fresh. */
export const RESEND_MS = 30 * 24 * 60 * 60 * 1000;

export function shouldSend(sent: SentToken | null, wanted: Omit<SentToken, 'at'>, now: number): boolean {
  return (
    !sent ||
    sent.device !== wanted.device ||
    sent.tokenSha256 !== wanted.tokenSha256 ||
    sent.app !== wanted.app ||
    sent.apnsEnvironment !== wanted.apnsEnvironment ||
    now - sent.at > RESEND_MS
  );
}

/** Dev builds have their own package, Firebase project and bundle id, and reach a server that serves only them. */
function pushApp(): PushApp {
  return Application.applicationId?.endsWith('.dev') ? 'dev' : 'production';
}

async function readSent(): Promise<SentToken | null> {
  const stored = await SecureStore.getItemAsync(ITEM);
  if (!stored) return null;
  try {
    return JSON.parse(stored) as SentToken;
  } catch {
    return null;
  }
}

export async function notificationsAllowed(): Promise<boolean> {
  return (await Notifications.getPermissionsAsync()).granted;
}

let syncing: Promise<void> | undefined;

/**
 * Makes the server's copy of this phone's token match what the person chose and the system allows:
 * sends the token when it is new, changed or old, and takes it back when notifications are off.
 * Android's token is FCM's; an iPhone's is Apple's own, from the push server its build was signed for.
 */
export function syncPushToken(token: TokenSource): Promise<void> {
  syncing ??= (async () => {
    if (!notifier) return;
    const sent = await readSent();
    const wanted = (await notificationsChoice()) === 'on' && (await notificationsAllowed());
    if (!wanted) {
      if (sent) await stopPush(token);
      return;
    }
    const device = await deviceIdentity();
    notifier.setPhone(device.id());
    const { data } = await Notifications.getDevicePushTokenAsync();
    const pushToken = String(data);
    const tokenSha256 = await Crypto.digestStringAsync(Crypto.CryptoDigestAlgorithm.SHA256, pushToken);
    const apnsEnvironment = notifier.apnsEnvironment?.();
    const next = { device: device.id(), tokenSha256, app: pushApp(), ...(apnsEnvironment ? { apnsEnvironment } : {}) };
    if (!shouldSend(sent, next, Date.now())) return;
    await setPushToken(token, { token: pushToken, tokenSha256, app: next.app, ...(apnsEnvironment ? { apnsEnvironment } : {}) });
    await SecureStore.setItemAsync(ITEM, JSON.stringify({ ...next, at: Date.now() }));
  })().finally(() => {
    syncing = undefined;
  });
  return syncing;
}

/** Takes this phone's token off the server. Throws when the server can't be reached, so signing out can ask first. */
export async function stopPush(token: TokenSource): Promise<void> {
  if (!notifier) return;
  await clearPushToken(token);
  await SecureStore.deleteItemAsync(ITEM);
}

/**
 * Forgets everything notifications kept on this phone, as signing out does, and has the system drop its
 * token, so a server that never heard of the sign-out has nothing left to send to.
 */
export async function forgetPush(): Promise<void> {
  notifier?.removeAll();
  await SecureStore.deleteItemAsync(ITEM);
  if (notifier) await Notifications.unregisterForNotificationsAsync();
}
