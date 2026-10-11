import { AppState } from 'react-native';
import { MobileError, type ConnectionLike, type CoreListener } from '@sikemux/native';

import { notifier, type AnswerOutcome, type ShownCard } from '../../modules/notify';
import type { Snapshot } from '@/core/protocol';
import { goOffline, whileJoining } from '@/device/identity';
import { openConnections } from '@/devices/hub';

/** Permission cards whose request the host no longer has: it was answered or withdrawn elsewhere. */
export function settledCards(shown: ShownCard[], host: string, snapshot: Snapshot): string[] {
  const pending = new Set(snapshot.attentions.map((attention) => attention.id));
  return shown
    .filter((card) => card.host === host && card.kind === 'permission' && card.request && !pending.has(card.request))
    .map((card) => card.tag);
}

/** Removes the shown cards that `pick` chooses. */
function dismissShown(pick: (shown: ShownCard[]) => string[]) {
  if (!notifier) return;
  notifier
    .shown()
    .then((shown) => pick(shown).forEach((tag) => notifier?.dismiss(tag)))
    .catch((error: unknown) => console.warn('sikemux: could not tidy the notification cards', error));
}

/** The permission requests each host's last view held, so a view that settles none needs no look at the cards. */
const pendingSeen = new Map<string, Set<string>>();

/**
 * Removes the cards a host's current view shows are settled, as the app hears it. Listing the shown cards is a
 * trip into the system, so it happens only when a request went away, or on a connection's first view, which may
 * settle cards that arrived while the phone was away.
 */
export function reconcileCards(host: string, snapshot: Snapshot, fresh: boolean) {
  const pending = new Set(snapshot.attentions.map((attention) => attention.id));
  const before = pendingSeen.get(host);
  pendingSeen.set(host, pending);
  if (!fresh && before && [...before].every((id) => pending.has(id))) return;
  dismissShown((shown) => settledCards(shown, host, snapshot));
}

/** Removes the cards about one agent, once the person is looking at it. */
export function dismissCardsFor(host: string, agent: string) {
  dismissShown((shown) => shown.filter((card) => card.host === host && card.agent === agent).map((card) => card.tag));
}

/** Removes every card from a host, as when it is forgotten. */
export function dismissHostCards(host: string) {
  dismissShown((shown) => shown.filter((card) => card.host === host).map((card) => card.tag));
}

export type CardAnswer = { tag: string; host: string; agent: string; request: string; option: string; allow: boolean };

const quiet: CoreListener = {
  events() {},
  closed() {},
};

export function isGone(error: unknown): boolean {
  return MobileError.Refused.instanceOf(error) && /no longer pending/.test(error.inner.message);
}

/**
 * Answers a permission request from its card: over the connection the app already holds to the host, or else over
 * one of its own that keeps the phone online until the answer is in.
 */
export async function answerFromCard(answer: CardAnswer): Promise<AnswerOutcome> {
  const send = (connection: ConnectionLike) => connection.answerPermission(answer.agent, answer.request, answer.option);
  try {
    const held = openConnections().find(([host]) => host === answer.host)?.[1];
    if (held) await send(held);
    else {
      await whileJoining(async (device) => {
        const connection = await device.connect(answer.host, quiet);
        try {
          await send(connection);
        } finally {
          connection.close();
        }
      });
    }
    return answer.allow ? 'answered' : 'rejected';
  } catch (error) {
    if (isGone(error)) return 'gone';
    console.warn('sikemux: could not answer from the notification', error);
    return 'failed';
  }
}

/** Answers from a card's Allow or Reject: Android runs it without a screen, iOS once the button has opened the app. */
export async function answerTask(data: CardAnswer) {
  const outcome = await answerFromCard(data);
  notifier?.settle(data.tag, outcome);
  // Started for this answer alone, the phone would otherwise stay on the network with no app to use it.
  if (AppState.currentState !== 'active' && openConnections().length === 0) await goOffline();
}
