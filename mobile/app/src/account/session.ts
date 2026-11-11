import { useEffect, useEffectEvent, useRef } from 'react';
import { Alert, AppState } from 'react-native';
import { useAuth } from '@clerk/expo';
import { nativeApplicationVersion } from 'expo-application';
import * as Notifications from 'expo-notifications';
import type { Device } from '@sikemux/protocol';
import { liveUrl } from '@shared/account/live';

import { externalValue, useExternalValue } from '@shared/lib/externalValue';

import { problem as problemOf } from '@/core/problem';
import { alertFailure } from '@/ui/prompt';
import { notifier } from '../../modules/notify';
import { deviceIdentity } from '@/phone/identity';
import { onNetworkChange } from '@/network/connectivity';
import { phoneAdded, syncPushToken } from '@/notify/token';
import { stashChanged } from '@/stash/store';
import { accountDevice, accountHosts, registerPhone, removeAccountDevice, type TokenSource } from './api';
import { apiUrl } from './config';
import { farewellFor } from './farewell';
import { signOutHere } from './leave';
import { LiveAccount, runLive, savedCursor } from './live';
import { AccountSequence, type AccountStatus } from './sequence';

/** Counts the times the live connection heard the account's hosts change, each one asking for them again. */
const hostsVersion = externalValue(0);

function hostsChanged() {
  hostsVersion.set(hostsVersion.get() + 1);
}

/** The hosts on the account; `loaded` once the server has answered since signing in, `problem` while the last read failed. */
type AccountHosts = { hosts: Device[]; loaded: boolean; problem?: string };

const NO_HOSTS: AccountHosts = { hosts: [], loaded: false };
const accountHostsNow = externalValue(NO_HOSTS);

/** Reads the hosts on the account on signing in, and again whenever the live connection hears one change. */
export function useAccountHostsFeed() {
  const { isSignedIn, getToken } = useAuth();
  const version = useExternalValue(hostsVersion);
  const latestGetToken = useRef(getToken);
  useEffect(() => {
    latestGetToken.current = getToken;
  });
  useEffect(() => {
    if (!isSignedIn) {
      accountHostsNow.set(NO_HOSTS);
      return;
    }
    let current = true;
    accountHosts(() => latestGetToken.current())
      .then((hosts) => current && accountHostsNow.set({ hosts, loaded: true }))
      .catch((error: unknown) => {
        console.warn('sikemux: could not list the hosts on the account', error);
        if (current) accountHostsNow.set({ ...accountHostsNow.get(), problem: problemOf(error) });
      });
    return () => {
      current = false;
    };
  }, [isSignedIn, version]);
}

/** Reads the hosts again, unless the last read worked; the live connection keeps a good list current. */
export function retryHosts() {
  const now = accountHostsNow.get();
  if (!now.loaded || now.problem) hostsChanged();
}

export function useAccountHosts(): AccountHosts {
  return useExternalValue(accountHostsNow);
}

const statusNow = externalValue<AccountStatus | undefined>(undefined);
let running: AccountSequence | undefined;

/** How far adding this phone to the account has got; undefined while signed out. */
export function useAccountStatus(): AccountStatus | undefined {
  return useExternalValue(statusNow);
}

/** Signing in is enough to reach every host, so a device that joins the account is shown, with one tap to take it off (ADR 0018). */
async function askAboutNewDevice(token: TokenSource, key: string) {
  const device = await accountDevice(token, key).catch(() => undefined);
  if (!device) return;
  Alert.alert('New device on your account', device.name, [
    { text: 'Keep', style: 'cancel' },
    { text: 'Remove', style: 'destructive', onPress: () => void removeAccountDevice(token, key).catch(alertFailure("Couldn't remove it")) },
  ]);
}

/** Tries adding this phone to the account again now, as the person asked. */
export function retryRegistration() {
  running?.retryNow();
}

/**
 * Puts this phone on the account while signed in, and keeps it connected while the app is in front:
 * registration, then the notification token, then the live connection, as AccountSequence orders them.
 */
export function useAccountSequence() {
  const { isSignedIn, userId, getToken, signOut } = useAuth();
  const token = useEffectEvent(() => getToken());
  const leave = useEffectEvent((farewell: ReturnType<typeof farewellFor>) => signOutHere(() => signOut(), { farewell, confirmed: true }));
  useEffect(() => {
    if (!isSignedIn || !userId) return;
    const live = new LiveAccount({
      url: liveUrl(apiUrl()),
      connect: (url, on) => {
        const socket = new WebSocket(url);
        socket.onmessage = (event) => on.message(event.data);
        socket.onclose = (event) => on.closed(event.code);
        return socket;
      },
      key: async () => (await deviceIdentity()).id(),
      sign: async (nonce) => (await deviceIdentity()).signLive(nonce),
      token: () => token(),
      app: { platform: 'android', version: nativeApplicationVersion ?? 'unknown' },
      cursor: savedCursor,
      hostsChanged,
      stashChanged,
      deviceAdded: (key) => void askAboutNewDevice(() => token(), key),
      ready: retryHosts,
      gone: (reason) => void leave(farewellFor(reason)),
    });
    runLive(live);
    const sequence = new AccountSequence({
      register: async () => {
        const phone = await registerPhone(() => token(), userId);
        await phoneAdded(phone.createdAt);
      },
      syncPush: () => syncPushToken(() => token()),
      live,
      hostsStale: retryHosts,
      status: statusNow.set,
    });
    running = sequence;
    sequence.start(AppState.currentState === 'active');
    const following = AppState.addEventListener('change', (state) => {
      if (state === 'active') sequence.foreground();
      else if (state === 'background') sequence.background();
    });
    const network = onNetworkChange(() => sequence.online());
    const renewed = notifier ? Notifications.addPushTokenListener(() => sequence.pushChanged()) : undefined;
    return () => {
      following.remove();
      network();
      renewed?.remove();
      sequence.stop();
      if (running === sequence) running = undefined;
      statusNow.set(undefined);
    };
  }, [isSignedIn, userId]);
}
