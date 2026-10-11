import { describe, expect, it } from 'vitest';

import { ChatState, type ChatInfo } from '@/core/protocol';
import { age, ago, chatState, folder } from '@/devices/words';

function chat(overrides: Partial<ChatInfo> = {}): ChatInfo {
  return {
    agentId: 'agent-1',
    provider: 'claude',
    cwd: '/Users/me/project',
    state: ChatState.Ready,
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
    expect(chatState(chat({ asleep: true, state: ChatState.Stopped }))).toBe('Sleeping');
  });

  it('follows the agent through its lifecycle', () => {
    expect(chatState(chat({ state: ChatState.Starting }))).toBe('Starting…');
    expect(chatState(chat({ state: ChatState.Ready, running: true }))).toBe('Working');
    expect(chatState(chat({ state: ChatState.Ready }))).toBe('Ready');
    expect(chatState(chat({ state: ChatState.Stopped, running: true }))).toBe('Stopped');
  });
});

describe('ago', () => {
  const now = Date.UTC(2026, 9, 2, 12);
  const minutes = (count: number) => now - count * 60_000;

  it('rounds to the nearest unit', () => {
    expect(ago(now, now)).toBe('just now');
    expect(ago(minutes(0.4), now)).toBe('just now');
    expect(ago(minutes(1), now)).toBe('1m ago');
    expect(ago(minutes(59), now)).toBe('59m ago');
    expect(ago(minutes(60), now)).toBe('1h ago');
    expect(ago(minutes(23 * 60), now)).toBe('23h ago');
    expect(ago(minutes(24 * 60), now)).toBe('1d ago');
    expect(ago(minutes(10 * 24 * 60), now)).toBe('10d ago');
  });

  it('says just now for a time a little in the future', () => {
    expect(ago(now + 20_000, now)).toBe('just now');
  });

  it('is as brief as the rail without the ago', () => {
    expect(age(minutes(0.4), now)).toBe('now');
    expect(age(minutes(3 * 60), now)).toBe('3h');
    expect(age(minutes(24 * 60), now)).toBe('1d');
  });
});
