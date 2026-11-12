import { createContext } from 'react';

import type { Subagents } from '@shared/chat/subagents';
import type { ChatMessage } from '@shared/wire.generated';

/** The chat's agent, whose mark its subagents carry. */
export const ProviderContext = createContext('agent');

/** The chat's own agent, which an answer to its own question needs no name for. */
export const AgentContext = createContext('');

export const SubagentsContext = createContext<Subagents>({ card: () => undefined, rows: {} });

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

/** Whether this phone may act on the host, as Overrule's Undo needs; a watching phone or a shared chat may not. */
export const ActsContext = createContext(false);
