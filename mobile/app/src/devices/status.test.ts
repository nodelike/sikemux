import { describe, expect, it } from 'vitest';
import type { ConnectionLike } from '@sikemux/native';

import type { Snapshot } from '@/core/protocol';
import { hostStatus } from './status';

const NOW = Date.UTC(2026, 9, 6, 12);
const SNAPSHOT = { chats: [{}, {}], sessions: [{ running: true }, { running: false }] } as unknown as Snapshot;
const connection = {} as ConnectionLike;
const device = { channel: 'nightly' as const, lastSeen: NOW - 5 * 60_000 };

describe('a host status', () => {
  it('is online only while connected, naming the channel and what runs there', () => {
    expect(hostStatus({ status: 'open', connection, snapshot: SNAPSHOT }, device, NOW)).toMatchObject({
      online: true,
      stale: false,
      line: 'Online · Nightly · 2 agents · 1 terminal',
      problem: null,
    });
  });

  it('shows an old view while reconnecting as stale, not online', () => {
    expect(hostStatus({ status: 'connecting', snapshot: SNAPSHOT }, device, NOW)).toMatchObject({
      online: false,
      connecting: true,
      stale: true,
      line: 'Reconnecting…',
    });
    expect(hostStatus({ status: 'connecting' }, device, NOW)).toMatchObject({ stale: false, line: 'Connecting…' });
  });

  it('stays offline while it tries again after a failure, so the line does not flicker', () => {
    const closed = hostStatus({ status: 'closed', problem: 'no route', snapshot: SNAPSHOT }, device, NOW);
    const trying = hostStatus({ status: 'connecting', problem: 'no route', snapshot: SNAPSHOT }, device, NOW);
    expect(closed.line).toBe('Asleep or offline · seen 5m ago');
    expect(trying.line).toBe(closed.line);
    expect(trying.problem).toBe('no route');
    expect(hostStatus({ status: 'closed', problem: 'no route' }, { channel: 'nightly', lastSeen: NOW - 10_000 }, NOW).line).toBe(
      'Asleep or offline',
    );
  });

  it('says when the host no longer knows this phone, or one side needs updating', () => {
    expect(hostStatus({ status: 'closed', problem: 'gone', unpaired: true }, device, NOW)).toMatchObject({
      unpaired: true,
      line: 'No longer knows this phone',
    });
    expect(hostStatus({ status: 'closed', problem: 'old', outdated: 'host' }, device, NOW).line).toBe('Needs a newer Sikemux');
    expect(hostStatus({ status: 'closed', problem: 'old', outdated: 'phone' }, device, NOW).line).toBe('Update this app to connect');
  });
});
