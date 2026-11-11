import { createContext } from 'react';

import type { Subagents } from '@shared/chat/subagents';
import type { ChatMessage } from '@shared/wire.generated';

/** The chat's agent, whose mark its subagents carry. */
export const ProviderContext = createContext('agent');

/** The chat's own agent, which an answer to its own question needs no name for. */
export const AgentContext = createContext('');

export const SubagentsContext = createContext<Subagents>({ card: () => undefined, rows: {} });

/** The thoughts of a streaming turn before its latest, which fold behind one line where the first of them stood. */
export type EarlierThoughts = { key: string; ids: readonly string[] };

export const NO_EARLIER_THOUGHTS: EarlierThoughts = { key: '', ids: [] };

export const EarlierThoughtsContext = createContext<EarlierThoughts>(NO_EARLIER_THOUGHTS);

/** The streaming turn's thoughts but its latest, counted across its messages; nothing folds once the turn ends. */
export function earlierThoughts(messages: readonly ChatMessage[], running: boolean): EarlierThoughts {
  if (!running) return NO_EARLIER_THOUGHTS;
  const start = messages.findLastIndex((message) => message.role === 'user') + 1;
  const first = messages[start];
  if (!first) return NO_EARLIER_THOUGHTS;
  const ids = messages
    .slice(start)
    .flatMap((message) => message.parts)
    .filter((part) => part.kind === 'thought')
    .map((part) => part.id);
  ids.pop();
  return ids.length ? { key: `earlier/${first.id}`, ids } : NO_EARLIER_THOUGHTS;
}

/** The fold key of the strip holding the running call, which opens as `RunStrip` keys it: its message, then its first call. */
export function liveRunKey(messages: readonly ChatMessage[], activeTool: string | undefined): string | null {
  const message = messages.at(-1);
  if (!message || !activeTool) return null;
  const at = message.parts.findIndex((part) => part.id === activeTool);
  if (at < 0) return null;
  let head = at;
  while (head > 0) {
    const before = message.parts[head - 1];
    if (before.kind !== 'tool' || before.page || before.gif) break;
    head -= 1;
  }
  return `${message.id}/${message.parts[head].id}`;
}
