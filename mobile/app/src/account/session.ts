import { useEffect, useEffectEvent, useRef, useState, useSyncExternalStore } from 'react';
import { AppState, Platform } from 'react-native';
import { router } from 'expo-router';
import { useAuth } from '@clerk/expo';
import { nativeApplicationVersion } from 'expo-application';
import * as Notifications from 'expo-notifications';
import type { Device } from '@protocol';

import { notifier } from '../../modules/notify';
import { deviceIdentity } from '@/device/identity';
import { hostsArrived } from '@/devices/arrivals';
import { joinShowing } from '@/devices/joining';
import { pairedDevices } from '@/devices/paired';
import { onNetworkChange } from '@/network/connectivity';
import { syncPushToken } from '@/notify/token';
import { accountHosts, registerPhone } from './api';
import { apiUrl } from './config';
import { farewellFor } from './farewell';
import { signOutHere } from './leave';
import { LiveAccount, liveUrl, runLive, savedCursor } from './live';
import { AccountSequence, type AccountStatus } from './sequence';

let hostsVersion = 0;
const hostsListeners = new Set<() => void>();

function hostsChanged() {
  hostsVersion += 1;
  hostsListeners.forEach((listener) => listener());
}

function subscribeHosts(listener: () => void) {
  hostsListeners.add(listener);
  return () => hostsListeners.delete(listener);
}

/** The hosts on the account; `loaded` once the server has answered since signing in, `problem` while the last read failed. */
export type AccountHosts = { hosts: Device[]; loaded: boolean; problem?: string };

const NO_HOSTS: AccountHosts = { hosts: [], loaded: false };
let accountHostsNow = NO_HOSTS;

function publishHosts(next: AccountHosts) {
  accountHostsNow = next;
  hostsListeners.forEach((listener) => listener());
}

/** Reads the hosts on the account on signing in, and again whenever the live connection hears one change. */
export function useAccountHostsFeed() {
  const { isSignedIn, getToken } = useAuth();
  const version = useSyncExternalStore(subscribeHosts, () => hostsVersion);
  const latestGetToken = useRef(getToken);
  useEffect(() => {
    latestGetToken.current = getToken;
  });
  useEffect(() => {
    if (!isSignedIn) {
      publishHosts(NO_HOSTS);
      return;
    }
    let current = true;
    accountHosts(() => latestGetToken.current())
      .then((hosts) => current && publishHosts({ hosts, loaded: true }))
      .catch((error: unknown) => {
        console.warn('sikemux: could not list the hosts on the account', error);
        if (current) publishHosts({ ...accountHostsNow, problem: error instanceof Error ? error.message : String(error) });
      });
    return () => {
      current = false;
    };
  }, [isSignedIn, version]);
}

/** Reads the hosts again, unless the last read worked; the live connection keeps a good list current. */
export function retryHosts() {
  if (!accountHostsNow.loaded || accountHostsNow.problem) hostsChanged();
}

export function useAccountHosts(): AccountHosts {
  return useSyncExternalStore(subscribeHosts, () => accountHostsNow);
}

/** A host that arrives while the phone is in the background stays new until the phone is in front, so it still connects then. */
async function connectArrival(account: string, hosts: Device[]) {
  if (joinShowing() || AppState.currentState !== 'active') return;
  const arrived = await hostsArrived(account, hosts);
  const paired = await pairedDevices();
  const host = arrived.find((found) => !paired.some((device) => device.core === found.key));
  if (!host) return;
  router.push({ pathname: '/join', params: { core: host.key, name: host.name, arrived: '1' } });
}

/** Starts connecting to a host that signs in to the account after this phone did; the host may have let this phone in already. */
export function useConnectArrivals() {
  const { userId } = useAuth();
  const { hosts, loaded } = useAccountHosts();
  const [inFront, setInFront] = useState(0);
  useEffect(() => {
    const back = AppState.addEventListener('change', (state) => {
      if (state === 'active') setInFront((count) => count + 1);
    });
    return () => back.remove();
  }, []);
  useEffect(() => {
    if (!userId || !loaded) return;
    connectArrival(userId, hosts).catch((error: unknown) => console.warn('sikemux: could not connect to a new host', error));
  }, [userId, loaded, hosts, inFront]);
}

let statusNow: AccountStatus | undefined;
const statusListeners = new Set<() => void>();
let running: AccountSequence | undefined;

function publishStatus(next: AccountStatus | undefined) {
  statusNow = next;
  statusListeners.forEach((listener) => listener());
}

/** How far adding this phone to the account has got; undefined while signed out. */
export function useAccountStatus(): AccountStatus | undefined {
  return useSyncExternalStore(
    (listener) => {
      statusListeners.add(listener);
      return () => statusListeners.delete(listener);
    },
    () => statusNow,
  );
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
      app: { platform: Platform.OS === 'ios' ? 'ios' : 'android', version: nativeApplicationVersion ?? 'unknown' },
      cursor: savedCursor,
      hostsChanged,
      ready: retryHosts,
      gone: (reason) => void leave(farewellFor(reason)),
    });
    runLive(live);
    const sequence = new AccountSequence({
      register: async () => {
        await registerPhone(() => token(), userId);
      },
      syncPush: () => syncPushToken(() => token()),
      live,
      hostsStale: retryHosts,
      status: publishStatus,
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
      publishStatus(undefined);
    };
  }, [isSignedIn, userId]);
}
