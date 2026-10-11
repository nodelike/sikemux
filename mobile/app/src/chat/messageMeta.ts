import { rowMeta, type RowMeta } from '@mac/chat/messageMeta';
import type { ChatMessage } from '@mac/chat/types';

/**
 * The message a strip sits under: a prompt itself; an answer, which the agent splits into a message per tool call,
 * the last of its turn.
 */
export function stripOwner(messages: readonly ChatMessage[], id: string): string | undefined {
  let index = messages.findIndex((message) => message.id === id);
  if (index < 0) return undefined;
  if (messages[index].role === 'assistant') {
    while (messages[index + 1]?.role === 'assistant' && !messages[index + 1].opensTurn) index += 1;
  }
  return messages[index].id;
}

/**
 * What a message's strip says of it: a prompt its own text and when it was sent; an answer all of
 * its prose and how long the turn took.
 */
export function stripMeta(messages: readonly ChatMessage[], id: string): RowMeta | null {
  const index = messages.findIndex((message) => message.id === id);
  return index < 0 ? null : rowMeta(messages as ChatMessage[], index);
}
