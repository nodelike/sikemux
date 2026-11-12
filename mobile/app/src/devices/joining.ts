import { Platform } from 'react-native';
import { JoinAnswer, MobileError } from '@sikemux/native';

import { english, type Say } from '@shared/lib/say';
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
function refusal(host: string, reason: string, say: Say): JoinFailure {
  const title = say("{host} couldn't let this phone in", { host });
  const clock = () => say('Check the clock on {host} and on this phone, then try again.', { host });
  const why: Record<string, () => string> = {
    signed_out: () => say('{host} is not signed in to Sikemux. Sign it in, then try again.', { host }),
    wrong_account: () => say('{host} is signed in to a different account. Sign it in to this one, then try again.', { host }),
    revoked: () => say('This phone was removed from your account. Sign in again.'),
    expired: clock,
    not_yet_valid: clock,
    unknown_key: () => say('{host} does not trust this Sikemux server. Update it, then try again.', { host }),
  };
  return { title, detail: why[reason]?.() ?? say('It turned down the invitation ({reason}). Try again.', { reason }) };
}

function ticketFailure(host: string, error: unknown, say: Say): JoinFailure {
  if (error instanceof ApiProblem) {
    if (error.status === 404)
      return {
        title: say('{host} is no longer on your account', { host }),
        detail: say('Sign it in to this account, then try again.'),
      };
    if (error.status === 429) return { title: say('Too many tries'), detail: say('Wait a minute, then try again.') };
    if (error.unreachable) return { title: say("Can't reach Sikemux"), detail: say('Check the phone is online, then try again.') };
    return { title: say("Sikemux couldn't invite this phone"), detail: error.message };
  }
  return { title: say("Sikemux couldn't invite this phone"), detail: problem(error) };
}

function dialFailure(host: string, error: unknown, say: Say): JoinFailure {
  if (MobileError.Connection.instanceOf(error)) {
    return {
      title: say("Can't reach {host}", { host }),
      detail: say('It may be asleep, offline, or have remote access turned off.'),
    };
  }
  if (MobileError.Invalid.instanceOf(error)) return { title: say("Sikemux's invitation didn't fit"), detail: error.inner.message };
  return { title: say('Connecting stopped'), detail: problem(error) };
}

/**
 * Joins the account's host `core`: gets a ticket from Sikemux, hands it to the host and waits while it lets the phone
 * in, at once or once someone there allows it. An allowed phone keeps the host as paired. Aborting `signal` stops it.
 */
export async function joinHost(
  host: { core: string; name: string },
  token: TokenSource,
  signal?: AbortSignal,
  say: Say = english,
): Promise<Access> {
  const ticket = await joinTicket(token, host.core).catch((error: unknown) => {
    throw new JoinFailed(ticketFailure(host.name, error, say));
  });
  if (signal?.aborted) throw new Error('Connecting was cancelled.');
  const asked = Date.now();
  const answer = await whileJoining((device) =>
    device.join(host.core, JSON.stringify(ticket), phoneName(), Platform.OS, signal ? { signal } : undefined),
  ).catch((error: unknown) => {
    throw new JoinFailed(dialFailure(host.name, error, say));
  });
  if (signal?.aborted) throw new Error('Connecting was cancelled.');
  if (JoinAnswer.Denied.instanceOf(answer) && Date.now() - asked >= APPROVAL_TIMEOUT_SECONDS * 1000) {
    throw new JoinFailed({
      title: say('{host} did not answer', { host: host.name }),
      detail: say('Nobody allowed this phone within {minutes} minutes.', { minutes: APPROVAL_TIMEOUT_SECONDS / 60 }),
    });
  }
  if (JoinAnswer.Denied.instanceOf(answer)) {
    throw new JoinFailed({
      title: say('{host} said no', { host: host.name }),
      detail: say('Someone at {host} turned this phone down.', { host: host.name }),
      held: true,
    });
  }
  if (JoinAnswer.Refused.instanceOf(answer)) throw new JoinFailed(refusal(host.name, answer.inner.reason, say));
  const access = answer.inner.access as Access;
  await rememberDevice({ core: host.core, access, pairedAt: Date.now(), name: host.name });
  rejoined(host.core);
  await reloadDevices();
  return access;
}
