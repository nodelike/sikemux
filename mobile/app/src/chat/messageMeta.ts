import { copyText } from '@mac/chat/messageMeta';
import type { ChatMessage } from '@mac/chat/types';

/** What a strip under a message shows: when it was sent or its turn finished, how long that took, how fast it was written, and what Copy gives. */
export type Strip = NonNullable<ChatMessage['meta']> & { text: string };

/**
 * The message a strip sits under: a prompt itself; an answer, which the agent splits into a message per tool call,
 * the last of its turn, once that turn has finished.
 */
export function stripOwner(messages: readonly ChatMessage[], id: string): string | undefined {
  const index = messages.findIndex((message) => message.id === id);
  if (index < 0) return undefined;
  if (messages[index].role === 'user') return messages[index].meta ? id : undefined;
  for (let at = index; messages[at]?.role === 'assistant'; at += 1) {
    if (at > index && messages[at].opensTurn) return undefined;
    if (messages[at].meta) return messages[at].id;
  }
  return undefined;
}

export function stripOf(messages: readonly ChatMessage[], id: string): Strip | null {
  const index = messages.findIndex((message) => message.id === id);
  const meta = messages[index]?.meta;
  return meta ? { ...meta, text: copyText(messages, index) } : null;
}
