import { createContext } from 'react';

import type { Subagents } from '@shared/chat/subagents';

/** The chat's agent, whose mark its subagents carry. */
export const ProviderContext = createContext('agent');

/** The chat's own agent, which an answer to its own question needs no name for. */
export const AgentContext = createContext('');

export const SubagentsContext = createContext<Subagents>({ card: () => undefined, rows: {} });
