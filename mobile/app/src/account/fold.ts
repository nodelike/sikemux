import { useEffect } from 'react';
import { useAuth } from '@clerk/expo';
import * as SecureStore from 'expo-secure-store';
import type { FoldCatch } from '@sikemux/protocol';

import { externalValue, useExternalValue } from '@shared/lib/externalValue';

import { ApiProblem, api, type TokenSource } from './api';

/** A fold this phone started, waiting for the person's other account to confirm it. Kept in memory only. */
export type Waiting = { id: string; secret: string; expiresAt: string; waitingWords: string };

/**
 * Whether the signed-in account may go on to its screens. A new account may first be asked whether
 * the person already has one, and a fold this phone started is confirmed here.
 */
export type FoldGate =
  { kind: 'checking' } | { kind: 'open' } | { kind: 'catch'; caught: FoldCatch } | { kind: 'confirm'; waiting: Waiting };

const SIGNED_IN_HERE = 'sikemux.signed-in-here';
const settledItem = (userId: string) => `sikemux.one-account.${userId.replace(/[^A-Za-z0-9._-]/g, '')}`;

const GIVE_UP_AFTER = 3;
const TOO_MANY = 429;

const gateNow = externalValue<FoldGate>({ kind: 'checking' });
const waitingNow = externalValue<Waiting | null>(null);
const round = externalValue(0);
let unasked: { userId: string; failures: number } | null = null;

export function useFoldGate(): FoldGate {
  return useExternalValue(gateNow);
}

/** The fold waiting for the other account's sign-in. */
export function useWaitingFold(): Waiting | null {
  return useExternalValue(waitingNow);
}

export function holdFold(waiting: Waiting) {
  waitingNow.set(waiting);
}

export function dropFold() {
  waitingNow.set(null);
}

/** Asks the server again whether this account may stand, after it refused to add the phone. */
export function checkFoldAgain() {
  gateNow.set({ kind: 'checking' });
  round.set(round.get() + 1);
}

/** Lets the account in, and remembers it so it is not asked again. */
export async function openFold(userId: string) {
  gateNow.set({ kind: 'open' });
  await SecureStore.setItemAsync(settledItem(userId), '1').catch(() => {});
  await SecureStore.setItemAsync(SIGNED_IN_HERE, '1').catch(() => {});
}

async function askCatch(token: TokenSource) {
  const before = await SecureStore.getItemAsync(SIGNED_IN_HERE).catch(() => null);
  return api(token).getFoldCatch(before ? { hint: 'browser' } : {});
}

/**
 * Before the phone joins an account whose sign-in couldn't ask "Already on Sikemux?", asks it now, since
 * joining ends the offer. Throws while the server can't be reached, so joining waits and tries again.
 */
export async function askBeforeAddingPhone(token: TokenSource, userId: string) {
  if (unasked?.userId !== userId) return;
  try {
    const caught = await askCatch(token);
    unasked = null;
    if (caught.ask) {
      gateNow.set({ kind: 'catch', caught });
      throw new Error('This account asks the person first.');
    }
    await openFold(userId);
  } catch (error) {
    if (!(error instanceof ApiProblem) || !unasked) throw error;
    if (error.status === undefined) throw error;
    unasked.failures += 1;
    if ((error.unreachable || error.status === TOO_MANY) && unasked.failures < GIVE_UP_AFTER) throw error;
    console.warn('sikemux: adding the phone without asking whether this account may stand', error);
    unasked = null;
  }
}

/** Decides the gate on each sign-in, before the phone is added to the account. */
export function useFoldGateFeed() {
  const { isSignedIn, userId, getToken } = useAuth();
  const again = useExternalValue(round);
  useEffect(() => {
    if (!isSignedIn || !userId) {
      gateNow.set({ kind: 'checking' });
      return;
    }
    let current = true;
    void (async () => {
      const waiting = waitingNow.get();
      if (waiting && Date.parse(waiting.expiresAt) > Date.now()) {
        gateNow.set({ kind: 'confirm', waiting });
        return;
      }
      dropFold();
      if (again === 0 && (await SecureStore.getItemAsync(settledItem(userId)).catch(() => null))) {
        if (current) gateNow.set({ kind: 'open' });
        return;
      }
      try {
        const caught = await askCatch(() => getToken());
        if (!current) return;
        if (caught.ask) {
          gateNow.set({ kind: 'catch', caught });
          return;
        }
      } catch (error) {
        console.warn('sikemux: could not ask whether this account may stand', error);
        if (!current) return;
        unasked = { userId, failures: 0 };
        gateNow.set({ kind: 'open' });
        return;
      }
      if (current) await openFold(userId);
    })();
    return () => {
      current = false;
    };
  }, [isSignedIn, userId, getToken, again]);
}
