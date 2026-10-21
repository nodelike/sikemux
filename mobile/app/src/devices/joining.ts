import { useEffect } from 'react';
import { Platform } from 'react-native';
import { JoinAnswer, MobileError } from '@sikemux/native';

import { problem } from '@/core/problem';
import { inBackgroundTime } from '../../modules/background-time';
import { ApiProblem, joinTicket, type TokenSource } from '@/account/api';
import { whileJoining } from '@/device/identity';
import { phoneName } from '@/device/name';
import { rejoined, reloadDevices } from './hub';
import { rememberDevice, type Access } from './paired';

let screens = 0;

/** Whether a join screen is showing, so a host arriving meanwhile waits in Devices rather than taking over. */
export function joinShowing(): boolean {
  return screens > 0;
}

export function useJoinScreen() {
  useEffect(() => {
    screens += 1;
    return () => {
      screens -= 1;
    };
  }, []);
}

/** Asking Sikemux for a ticket, then waiting while the person at the host decides. */
export type JoinStep = 'asking' | 'waiting';

export type JoinFailure = { title: string; detail: string };

export class JoinFailed extends Error {
  constructor(readonly failure: JoinFailure) {
    super(failure.title);
  }
}

/** How long the host keeps a request open before it stops asking. */
export const APPROVAL_SECONDS = 120;

export function expired(host: string): JoinFailure {
  return { title: `${host} did not answer`, detail: 'Nobody allowed this phone within two minutes.' };
}

/** Why the host refused a ticket, in the words sikemux_core::join::Refusal uses. */
function refusal(host: string, reason: string): JoinFailure {
  const title = `${host} couldn't let this phone in`;
  const why: Record<string, string> = {
    signed_out: `${host} is not signed in to Sikemux. Sign it in, then try again.`,
    wrong_account: `${host} is signed in to a different account. Sign it in to this one, then try again.`,
    revoked: 'This phone was removed from your account. Sign in again.',
    expired: `Check the clock on ${host} and on this phone, then try again.`,
    not_yet_valid: `Check the clock on ${host} and on this phone, then try again.`,
    unknown_key: `${host} does not trust this Sikemux server. Update it, then try again.`,
  };
  return { title, detail: why[reason] ?? `It turned down the invitation (${reason}). Try again.` };
}

function ticketFailure(host: string, error: unknown): JoinFailure {
  if (error instanceof ApiProblem) {
    if (error.status === 404)
      return { title: `${host} is no longer on your account`, detail: 'Sign it in to this account, then try again.' };
    if (error.status === 429) return { title: 'Too many tries', detail: 'Wait a minute, then try again.' };
    if (error.unreachable) return { title: "Can't reach Sikemux", detail: 'Check the phone is online, then try again.' };
    return { title: "Sikemux couldn't invite this phone", detail: error.message };
  }
  return { title: "Sikemux couldn't invite this phone", detail: problem(error) };
}

function dialFailure(host: string, error: unknown): JoinFailure {
  if (MobileError.Connection.instanceOf(error)) {
    return { title: `Can't reach ${host}`, detail: 'It may be asleep, offline, or have remote access turned off.' };
  }
  if (MobileError.Invalid.instanceOf(error)) return { title: "Sikemux's invitation didn't fit", detail: error.inner.message };
  return { title: 'Connecting stopped', detail: problem(error) };
}

/**
 * Joins the account's host `core`: gets a ticket from Sikemux, hands it to the host and waits while someone there
 * decides. An allowed phone keeps the host as paired. Aborting `signal` stops it.
 */
export async function joinHost(
  host: { core: string; name: string },
  token: TokenSource,
  onStep: (step: JoinStep) => void,
  signal?: AbortSignal,
): Promise<Access> {
  onStep('asking');
  const ticket = await joinTicket(token, host.core).catch((error: unknown) => {
    throw new JoinFailed(ticketFailure(host.name, error));
  });
  if (signal?.aborted) throw new Error('Connecting was cancelled.');
  onStep('waiting');
  const answer = await inBackgroundTime('Joining a host', () =>
    whileJoining((device) => device.join(host.core, JSON.stringify(ticket), phoneName(), Platform.OS, signal ? { signal } : undefined)),
  ).catch((error: unknown) => {
    throw new JoinFailed(dialFailure(host.name, error));
  });
  if (signal?.aborted) throw new Error('Connecting was cancelled.');
  if (JoinAnswer.Denied.instanceOf(answer)) {
    throw new JoinFailed({ title: `${host.name} said no`, detail: `Someone at ${host.name} turned this phone down.` });
  }
  if (JoinAnswer.Refused.instanceOf(answer)) throw new JoinFailed(refusal(host.name, answer.inner.reason));
  const access = answer.inner.access as Access;
  await rememberDevice({ core: host.core, access, pairedAt: Date.now(), name: host.name });
  rejoined(host.core);
  await reloadDevices();
  return access;
}
