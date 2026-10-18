import { agentLabel } from '@mac/agents/catalog';
import { agentDisplay, type AgentDisplay } from '@mac/state/agentStatus';
import type { ChatInfo, InboxItem, ProjectInfo } from '@/core/protocol';

export function folder(path: string): string {
  return path.split('/').filter(Boolean).pop() ?? path;
}

/** The title the agent gave the chat, as the Mac's rail shows it. */
export function chatTitle(chat: Pick<ChatInfo, 'title' | 'provider'>): string {
  return chat.title ?? `New ${agentLabel(chat.provider)} chat`;
}

/**
 * Where a chat's turn is, grouped and counted as the Mac's rail does (src/state/agentStatus.ts). The core's group says
 * when it waits on the person to answer its question.
 */
export function chatDisplay(chat: ChatInfo): AgentDisplay {
  const blocked = chat.pendingPermissions.length > 0 || chat.group === 'blocked';
  const state = blocked ? 'blocked' : chat.running ? 'working' : chat.unread ? 'done' : 'idle';
  return agentDisplay(state, { subagents: chat.subagents, tasks: chat.tasks });
}

export function inProject(project: ProjectInfo, path: string): boolean {
  return path === project.path || path.startsWith(`${project.path}/`);
}

/** The item's card is on `project`'s board, or its agent works there. */
export function itemIn(item: InboxItem, project: ProjectInfo): boolean {
  const where = item.card?.project ?? item.project;
  return where !== undefined && (where === project.id || inProject(project, where));
}
