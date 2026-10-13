import { agentLabel } from '@mac/agents/catalog';
import { agentDisplay, type AgentDisplay } from '@mac/state/agentStatus';
import type { ChatInfo } from '@/core/protocol';

export function folder(path: string): string {
  return path.split('/').filter(Boolean).pop() ?? path;
}

/** The title the agent gave the chat, as the Mac's rail shows it. */
export function chatTitle(chat: Pick<ChatInfo, 'title' | 'provider'>): string {
  return chat.title ?? `New ${agentLabel(chat.provider)} chat`;
}

/** Where a chat's turn is, grouped and counted as the Mac's rail does (src/state/agentStatus.ts). */
export function chatDisplay(chat: ChatInfo): AgentDisplay {
  const state = chat.pendingPermissions.length ? 'blocked' : chat.running ? 'working' : chat.unread ? 'done' : 'idle';
  return agentDisplay(state, { subagents: chat.subagents, tasks: chat.tasks });
}

/** The Mac rail's words for where a chat is, plus the phone's own for a chat that is not running yet. */
export function chatState(chat: ChatInfo): string {
  const display = chatDisplay(chat);
  if (display.group === 'blocked') return display.label;
  if (chat.asleep) return 'Sleeping';
  if (chat.state === 'stopped') return 'Stopped';
  if (chat.state === 'starting') return 'Starting…';
  return display.label;
}
