import { useEffect } from 'react';
import { router, type Href } from 'expo-router';
import * as Notifications from 'expo-notifications';

import { answerTask, type CardAnswer } from './cards';

/** What the iPhone's notification extension keeps on a card it opened. */
type CardInfo = {
  host: string;
  hostName: string;
  agent: string;
  kind: string;
  url: string;
  request?: string;
  allow?: string;
  reject?: string;
};

/** What tapping a card asks for: its chat to open, and an answer when Allow or Reject was tapped. */
export type CardTap = { path: string | null; answer: CardAnswer | null };

/** The screen a host's link names: `sikemux://device/<host>/chat/<agent>` opens that chat in this build. */
export function cardPath(url: string): string | null {
  const match = /^sikemux:\/\/(device\/.+)$/.exec(url);
  return match ? `/${match[1]}` : null;
}

function cardInfo(data: Record<string, unknown>): CardInfo | null {
  const card = data.sikemux;
  if (typeof card !== 'object' || card === null) return null;
  const { host, agent, url } = card as Partial<CardInfo>;
  return typeof host === 'string' && typeof agent === 'string' && typeof url === 'string' ? (card as CardInfo) : null;
}

/** Null for a card the phone could not read, which only opens the app. */
export function cardTap(data: Record<string, unknown>, action: string, identifier: string): CardTap | null {
  const card = cardInfo(data);
  if (!card) return null;
  const tag = typeof data.c === 'string' ? data.c : identifier;
  const allow = action === 'allow';
  const option = allow ? card.allow : action === 'reject' ? card.reject : undefined;
  return {
    path: cardPath(card.url),
    answer: option && card.request ? { tag, host: card.host, agent: card.agent, request: card.request, option, allow } : null,
  };
}

// On iOS a card that arrives while the app is open shows over it, as it does on Android.
Notifications.setNotificationHandler({
  handleNotification: async () => ({ shouldShowBanner: true, shouldShowList: true, shouldPlaySound: true, shouldSetBadge: false }),
});

/** A push's `data` is only the `body` part of what it carries, so the card the extension adds is read from the whole of it. */
function cardData(request: Notifications.NotificationRequest): Record<string, unknown> {
  const { trigger } = request;
  if (trigger && 'type' in trigger && trigger.type === 'push' && trigger.payload) return trigger.payload;
  return request.content.data ?? {};
}

function respond(response: Notifications.NotificationResponse) {
  const { request } = response.notification;
  const tap = cardTap(cardData(request), response.actionIdentifier, request.identifier);
  if (!tap) return;
  if (tap.path) router.push(tap.path as Href);
  if (tap.answer) answerTask(tap.answer).catch((error: unknown) => console.warn('sikemux: could not answer from the notification', error));
}

/**
 * Opens the chat a tapped card is about, and sends the answer when it was Allow or Reject. Android's cards open
 * their links and answer by themselves; these are the iPhone's, whose buttons bring the app up to answer.
 */
export function useNotificationResponses() {
  useEffect(() => {
    const handled = new Set<string>();
    const handle = (response: Notifications.NotificationResponse) => {
      const key = `${response.notification.request.identifier}/${response.actionIdentifier}`;
      if (handled.has(key)) return;
      handled.add(key);
      Notifications.clearLastNotificationResponse();
      respond(response);
    };
    const launched = Notifications.getLastNotificationResponse();
    if (launched) handle(launched);
    const following = Notifications.addNotificationResponseReceivedListener(handle);
    return () => following.remove();
  }, []);
}
