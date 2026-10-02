import { ChatState, type ChatInfo } from '@/core/protocol';

const PROVIDER_NAMES: Record<string, string> = {
  claude: 'Claude Code',
  codex: 'Codex',
  hermes: 'Hermes',
  opencode: 'OpenCode',
  pi: 'Pi',
  omp: 'OMP',
  grok: 'Grok',
};

export function providerName(provider: string): string {
  return PROVIDER_NAMES[provider] ?? provider;
}

export function folder(path: string): string {
  return path.split('/').filter(Boolean).pop() ?? path;
}

/** The title the agent gave the chat, as the Mac's rail shows it. */
export function chatTitle(chat: Pick<ChatInfo, 'title' | 'provider'>): string {
  return chat.title ?? `New ${providerName(chat.provider)} chat`;
}

/** The Mac rail's words for where a chat is (src/state/agentStatus.ts). */
export function chatState(chat: ChatInfo): string {
  if (chat.pendingPermissions.length) return 'Needs input';
  if (chat.asleep) return 'Sleeping';
  if (chat.state === ChatState.Stopped) return 'Stopped';
  if (chat.state === ChatState.Starting) return 'Starting…';
  return chat.running ? 'Working' : 'Ready';
}

export function ago(at: number, now = Date.now()): string {
  const minutes = Math.round((now - at) / 60_000);
  if (minutes < 1) return 'just now';
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.round(hours / 24)}d ago`;
}
