import { useEffect, useRef, useState } from 'react';
import { AppState } from 'react-native';
import { useAuth } from '@clerk/expo';
import { File, Paths } from 'expo-file-system';
import type { Device } from '@sikemux/protocol';

import { externalValue, useExternalValue } from '@shared/lib/externalValue';

import { problem } from '@/core/problem';
import type { TokenSource } from '@/account/api';
import { useAccountHosts } from '@/account/session';
import { translate } from '@/i18n';
import { offerNotifications } from '@/notify/setting';
import { readWhole, writeWhole } from './disk';
import { useDevices } from './hub';
import { JoinFailed, joinHost, type JoinFailure } from './joining';

/** Joining a host on the account this phone has not paired with: since when, or why it stopped. */
export type HostJoin = { joining: number } | { stopped: JoinFailure };

/** A host that has not answered by now is most likely waiting for someone there to allow the phone. */
export const ASKING_AFTER_MS = 5000;

type Held = { account: string; hosts: Record<string, JoinFailure> };

const joins = externalValue<Record<string, HostJoin>>({});
const running = new Map<string, AbortController>();
const heldFile = new File(Paths.document, 'held-hosts.json');
let account: string | undefined;
let token: TokenSource = async () => null;

function isHeld(value: unknown): value is Held {
  const held = value as Held | null;
  return typeof held?.account === 'string' && typeof held.hosts === 'object' && held.hosts !== null;
}

function setJoin(core: string, join: HostJoin | undefined) {
  const next = { ...joins.get() };
  if (join) next[core] = join;
  else delete next[core];
  joins.set(next);
  saveHeld();
}

function saveHeld() {
  if (!account) return;
  const hosts: Record<string, JoinFailure> = {};
  for (const [core, join] of Object.entries(joins.get())) if ('stopped' in join && join.stopped.held) hosts[core] = join.stopped;
  writeWhole(heldFile, JSON.stringify({ account, hosts } satisfies Held));
}

/** Clears what another account left, then reads which of this one's hosts stay stopped until the person asks. */
async function followAccount(next: string | undefined) {
  if (next === account) return;
  running.forEach((controller) => controller.abort());
  running.clear();
  account = next;
  joins.set({});
  if (!next) return;
  const held = (await readWhole(heldFile, isHeld)).value;
  if (account !== next || held?.account !== next) return;
  const stopped = Object.fromEntries(Object.entries(held.hosts).map(([core, failure]) => [core, { stopped: failure }]));
  joins.set({ ...stopped, ...joins.get() });
}

function startJoin(host: Device) {
  const controller = new AbortController();
  running.set(host.key, controller);
  setJoin(host.key, { joining: Date.now() });
  joinHost({ core: host.key, name: host.name }, () => token(), controller.signal, translate)
    .then(() => {
      if (controller.signal.aborted) return;
      running.delete(host.key);
      setJoin(host.key, undefined);
      offerNotifications().catch(() => {});
    })
    .catch((error: unknown) => {
      if (controller.signal.aborted) return;
      running.delete(host.key);
      setJoin(host.key, {
        stopped: error instanceof JoinFailed ? error.failure : { title: translate('Connecting stopped'), detail: problem(error) },
      });
    });
}

export function useHostJoin(core: string): HostJoin | undefined {
  return useExternalValue(joins, (all) => all[core]);
}

export function retryJoin(host: Device) {
  if (!running.has(host.key)) startJoin(host);
}

/** The person forgot this host here, so it is not joined again by itself. */
export function forgotHost(core: string) {
  running.get(core)?.abort();
  running.delete(core);
  if (account)
    setJoin(core, {
      stopped: { title: translate('Forgotten on this phone'), detail: translate('Try again to use it here.'), held: true },
    });
}

/** Joins every host on the account this phone has not paired with, as soon as the account lists it (ADR 0018). */
export function useHostJoins() {
  const { userId, getToken } = useAuth();
  const { hosts, loaded } = useAccountHosts();
  const paired = useDevices();
  const [front, setFront] = useState(AppState.currentState !== 'background');
  const [ready, setReady] = useState<string>();
  const latestGetToken = useRef(getToken);
  useEffect(() => {
    latestGetToken.current = getToken;
    token = () => latestGetToken.current();
  });
  useEffect(() => {
    const following = AppState.addEventListener('change', (state) => setFront(state !== 'background'));
    return () => following.remove();
  }, []);
  useEffect(() => {
    let current = true;
    followAccount(userId ?? undefined)
      .catch((error: unknown) => console.warn('sikemux: could not read the hosts held back from joining', error))
      .finally(() => current && setReady(userId ?? undefined));
    return () => {
      current = false;
    };
  }, [userId]);
  useEffect(() => {
    if (!userId || ready !== userId || !loaded || !paired.loaded || !front) return;
    const all = joins.get();
    for (const host of hosts) {
      if (!all[host.key] && !paired.devices.some((device) => device.core === host.key)) startJoin(host);
    }
  }, [userId, ready, loaded, paired, front, hosts]);
}
