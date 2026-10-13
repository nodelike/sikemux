import { describe, expect, it } from 'vitest';

import type { ChatInfo } from '@/core/protocol';
import { chatState, folder } from '@/devices/words';

function chat(overrides: Partial<ChatInfo> = {}): ChatInfo {
  return {
    agentId: 'agent-1',
    provider: 'claude',
    cwd: '/Users/me/project',
    state: 'ready',
    running: false,
    pendingPermissions: [],
    subagents: 0,
    permissionMode: 'default',
    asleep: false,
    unread: false,
    ...overrides,
  };
}

describe('folder', () => {
  it('ignores a trailing slash', () => {
    expect(folder('/Users/me/project/')).toBe('project');
  });

  it('keeps the root as it is', () => {
    expect(folder('/')).toBe('/');
  });
});

describe('chatState', () => {
  it('puts a waiting permission ahead of everything', () => {
    expect(chatState(chat({ pendingPermissions: ['p1'], asleep: true, running: true }))).toBe('Needs input');
  });

  it('says a sleeping chat is asleep even when stopped', () => {
    expect(chatState(chat({ asleep: true, state: 'stopped' }))).toBe('Sleeping');
  });

  it('follows the agent through its lifecycle', () => {
    expect(chatState(chat({ state: 'starting' }))).toBe('Starting…');
    expect(chatState(chat({ state: 'ready', running: true }))).toBe('Working');
    expect(chatState(chat({ state: 'ready' }))).toBe('Idle');
    expect(chatState(chat({ state: 'ready', unread: true }))).toBe('Done');
    expect(chatState(chat({ state: 'stopped', running: true }))).toBe('Stopped');
  });
});
