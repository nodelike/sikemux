import { AppState, Platform, type AppStateStatus } from 'react-native';
import * as LocalAuthentication from 'expo-local-authentication';
import * as SecureStore from 'expo-secure-store';

import { externalValue, useExternalValue } from '@shared/lib/externalValue';
import { translate } from '@/i18n';

/** How long the app may be away before it asks again, as password managers allow a quick switch to another app. */
export const AWAY_MS = 60_000;

/**
 * open: showing the app. covered: away or behind a system sheet, so the app switcher's picture shows nothing.
 * locked: waiting for Face ID, Touch ID or the passcode.
 * `opened` turns true the first time the app shows itself after it starts. `on` is the person's choice, off until they turn it on.
 */
type LockState = { phase: 'open' | 'covered' | 'locked'; asking: boolean; opened: boolean; on: boolean };

const ITEM = 'sikemux.lock';
const guarded = Platform.OS !== 'web';
const active = () => AppState.currentState === 'active';

const lock = externalValue<LockState>(
  guarded
    ? { phase: active() ? 'locked' : 'covered', asking: false, opened: false, on: false }
    : { phase: 'open', asking: false, opened: true, on: false },
);
let locked = guarded;
/** Undefined until the account is known at launch. */
let signedIn: boolean | undefined;
let leftAt: number | undefined;
const waiting = new Set<() => void>();

const chosen: Promise<void> = guarded
  ? SecureStore.getItemAsync(ITEM)
      .then((stored) => set({ on: stored === 'on' }))
      .catch((error: unknown) => console.warn('sikemux: could not read whether the lock is on', error))
  : Promise.resolve();

function set(next: Partial<LockState>) {
  lock.set({ ...lock.get(), ...next });
  if (lock.get().phase !== 'open') return;
  waiting.forEach((resolve) => resolve());
  waiting.clear();
}

function open() {
  locked = false;
  leftAt = undefined;
  set({ phase: active() || !lock.get().on ? 'open' : 'covered', asking: false, opened: true });
}

/** Whether the person turned the lock on, so nothing outside the app shows what agents work on. */
export const lockOn = () => lock.get().on;
export const onLockChange = (listener: () => void) => lock.subscribe(listener);

export function useLock(): LockState {
  return useExternalValue(lock);
}

/** Resolves once the app shows itself again, so a link opened from outside waits for the lock. */
export function whenOpen(): Promise<void> {
  if (lock.get().phase === 'open') return Promise.resolve();
  return new Promise((resolve) => waiting.add(resolve));
}

/**
 * The lock keeps an account's hosts and chats. A phone signed in when the app starts asks first if the person
 * turned the lock on; signed out there is nothing behind it, and someone who signs in on the phone has just proved who they are.
 */
export async function lockFor(account: boolean) {
  if (!guarded || account === signedIn) return;
  const launched = signedIn === undefined;
  signedIn = account;
  await chosen;
  if (account && launched && lock.get().on) return;
  open();
}

/** Asks for Face ID, Touch ID or the phone's passcode, and opens the app once the person passes. */
export async function unlock(): Promise<void> {
  if (!locked || lock.get().asking || !active()) return;
  set({ asking: true });
  const result = await LocalAuthentication.authenticateAsync({ promptMessage: translate('Unlock Sikemux') }).catch(() => null);
  // A phone without a passcode has nothing to ask for, and anyone holding it already has everything on it.
  if (result?.success || result?.error === 'passcode_not_set' || result?.error === 'not_enrolled') {
    open();
    return;
  }
  set({ phase: 'locked', asking: false });
}

/** Turns the lock on once the person passes the prompt, so it is known to work; answers why when it can't. */
export async function turnLockOn(): Promise<string | undefined> {
  set({ asking: true });
  const result = await LocalAuthentication.authenticateAsync({ promptMessage: translate('Lock Sikemux') }).catch(() => null);
  set({ asking: false });
  if (!result) return undefined;
  if (!result.success)
    return result.error === 'passcode_not_set' || result.error === 'not_enrolled' ? 'Set a passcode in Settings first' : undefined;
  await SecureStore.setItemAsync(ITEM, 'on');
  set({ on: true });
  return undefined;
}

export async function turnLockOff() {
  await SecureStore.deleteItemAsync(ITEM);
  set({ on: false });
}

function changed(next: AppStateStatus) {
  const { asking, on } = lock.get();
  if (asking || !on) return;
  if (next !== 'active') {
    if (next === 'background') leftAt ??= Date.now();
    if (!locked) set({ phase: 'covered' });
    return;
  }
  if (signedIn && leftAt !== undefined && Date.now() - leftAt >= AWAY_MS) locked = true;
  leftAt = undefined;
  if (!locked) {
    set({ phase: 'open' });
    return;
  }
  set({ phase: 'locked' });
  void unlock();
}

if (guarded) AppState.addEventListener('change', changed);
