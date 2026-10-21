import { Platform } from 'react-native';
import type { ApnsEnvironment, Device, DeviceRegistration, JoinTicket, PushApp, PushTokenRegistration, PushTokenState } from '@protocol';
import { ApiProblem, connectApi } from '@protocol/api';

import { deviceIdentity } from '@/device/identity';
import { phoneName } from '@/device/name';
import { apiUrl } from './config';

export { ApiProblem };

export type TokenSource = () => Promise<string | null>;

/** The account was deleted and can still be restored, until `deletesAt`. */
export class AccountDeleting extends Error {
  constructor(readonly deletesAt: string) {
    super('This account is being deleted.');
  }
}

/** The API's routes, signed in with `token`. */
export const api = (token: TokenSource) => connectApi({ url: apiUrl(), token, offline: "Can't reach Sikemux. Check the phone is online." });

/**
 * Adds this phone to the account, proving it holds its key. Doing it again only updates its name.
 * A deleted account that can still be restored throws AccountDeleting.
 */
export async function registerPhone(token: TokenSource, userId: string): Promise<Device> {
  try {
    return await addPhone(token, userId);
  } catch (error) {
    if (!(error instanceof ApiProblem) || error.status !== 401) throw error;
    const account = await api(token)
      .getAccount()
      .catch(() => null);
    if (account?.status === 'deleting' && account.deletesAt) throw new AccountDeleting(account.deletesAt);
    throw error;
  }
}

async function addPhone(token: TokenSource, userId: string): Promise<Device> {
  const challenge = await api(token).createChallenge();
  const device = await deviceIdentity();
  const registration: DeviceRegistration = {
    key: device.id(),
    role: 'client',
    name: phoneName().slice(0, 64),
    platform: Platform.OS === 'ios' ? 'ios' : 'android',
    nonce: challenge.nonce,
    signature: device.signRegistration(challenge.nonce, userId),
  };
  return api(token).registerDevice(registration);
}

/** Takes this phone off the account, as signing out does. A phone the account no longer has is already off it. */
export async function removePhone(token: TokenSource): Promise<void> {
  const device = await deviceIdentity();
  try {
    await api(token).removeDevice(device.id());
  } catch (error) {
    if (error instanceof ApiProblem && error.status === 404) return;
    throw error;
  }
}

/** Sends this phone's notifications to its FCM or APNs token, proving the phone holds its key. */
export async function setPushToken(
  token: TokenSource,
  push: { token: string; tokenSha256: string; app: PushApp; apnsEnvironment?: ApnsEnvironment },
): Promise<PushTokenState> {
  const challenge = await api(token).createChallenge();
  const device = await deviceIdentity();
  const registration: PushTokenRegistration = {
    ...(push.apnsEnvironment ? { platform: 'apns', apnsEnvironment: push.apnsEnvironment } : { platform: 'fcm' }),
    token: push.token,
    app: push.app,
    nonce: challenge.nonce,
    signature: device.signPush(challenge.nonce, push.tokenSha256),
  };
  return api(token).setPushToken(device.id(), registration);
}

/** Stops the server sending this phone notifications. A phone the account no longer has gets none anyway. */
export async function clearPushToken(token: TokenSource): Promise<void> {
  const device = await deviceIdentity();
  try {
    await api(token).clearPushToken(device.id());
  } catch (error) {
    if (error instanceof ApiProblem && error.status === 404) return;
    throw error;
  }
}

/** The hosts signed in to the account. */
/** The account's device or host with this key, if it is still on the account. */
export async function accountDevice(token: TokenSource, key: string): Promise<Device | undefined> {
  return (await api(token).listDevices({})).devices.find((device) => device.key === key);
}

/** Takes a device or host off the account, which revokes it on every host at once. */
export async function removeAccountDevice(token: TokenSource, key: string): Promise<void> {
  await api(token).removeDevice(key);
}

export async function accountHosts(token: TokenSource): Promise<Device[]> {
  return (await api(token).listDevices({ role: 'host' })).devices;
}

/** The server's signed word that this phone and `host` are on the same account, for the host to check before it asks its owner. */
export async function joinTicket(token: TokenSource, host: string): Promise<JoinTicket> {
  const device = await deviceIdentity();
  return api(token).createJoinTicket(device.id(), { host });
}

/** Deletes the account and every device on it, with Apple's code for the server to revoke when it signs in with Apple. */
export async function deleteAccount(token: TokenSource, appleAuthorizationCode?: string) {
  return api(token).deleteAccount(appleAuthorizationCode ? { appleAuthorizationCode } : undefined);
}
