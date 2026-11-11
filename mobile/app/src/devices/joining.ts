import { Platform } from 'react-native';
import { JoinAnswer, MobileError } from '@sikemux/native';

import { APPROVAL_TIMEOUT_SECONDS } from '@shared/wire.generated';

import { problem } from '@/core/problem';
import { ApiProblem, joinTicket, type TokenSource } from '@/account/api';
import { whileJoining } from '@/phone/identity';
import { phoneName } from '@/phone/name';
import { rejoined, reloadDevices } from './hub';
import { rememberDevice, type Access } from './paired';

/** Why joining stopped, in words for the person. A `held` host is not joined again until the person asks. */
export type JoinFailure = { title: string; detail: string; held?: boolean };

export class JoinFailed extends Error {
  constructor(readonly failure: JoinFailure) {
    super(failure.title);
  }
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
 * Joins the account's host `core`: gets a ticket from Sikemux, hands it to the host and waits while it lets the phone
 * in, at once or once someone there allows it. An allowed phone keeps the host as paired. Aborting `signal` stops it.
 */
export async function joinHost(host: { core: string; name: string }, token: TokenSource, signal?: AbortSignal): Promise<Access> {
  const ticket = await joinTicket(token, host.core).catch((error: unknown) => {
    throw new JoinFailed(ticketFailure(host.name, error));
  });
  if (signal?.aborted) throw new Error('Connecting was cancelled.');
  const asked = Date.now();
  const answer = await whileJoining((device) =>
    device.join(host.core, JSON.stringify(ticket), phoneName(), Platform.OS, signal ? { signal } : undefined),
  ).catch((error: unknown) => {
    throw new JoinFailed(dialFailure(host.name, error));
  });
  if (signal?.aborted) throw new Error('Connecting was cancelled.');
  if (JoinAnswer.Denied.instanceOf(answer) && Date.now() - asked >= APPROVAL_TIMEOUT_SECONDS * 1000) {
    throw new JoinFailed({
      title: `${host.name} did not answer`,
      detail: `Nobody allowed this phone within ${APPROVAL_TIMEOUT_SECONDS / 60} minutes.`,
    });
  }
  if (JoinAnswer.Denied.instanceOf(answer)) {
    throw new JoinFailed({ title: `${host.name} said no`, detail: `Someone at ${host.name} turned this phone down.`, held: true });
  }
  if (JoinAnswer.Refused.instanceOf(answer)) throw new JoinFailed(refusal(host.name, answer.inner.reason));
  const access = answer.inner.access as Access;
  await rememberDevice({ core: host.core, access, pairedAt: Date.now(), name: host.name });
  rejoined(host.core);
  await reloadDevices();
  return access;
}
