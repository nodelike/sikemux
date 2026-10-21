import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from 'vitest';
import type { AccountEvent, LiveServerMessage, RevokeReason } from '@protocol';

import { afterByeDelay, liveUrl, reconnectDelay, type SocketHandlers } from '@protocol/live';

import { LiveAccount, readEvents, type LiveDeps } from './live';

const OWN = 'aa'.repeat(32);
const HOST = 'bb'.repeat(32);
const OTHER_HOST = 'cc'.repeat(32);
const NONCE = 'n'.repeat(64);

class FakeSocket {
  sent: Record<string, unknown>[] = [];
  closedWith?: number;
  constructor(readonly on: SocketHandlers) {}
  send(data: string) {
    this.sent.push(JSON.parse(data) as Record<string, unknown>);
  }
  close(code?: number) {
    this.closedWith = code ?? 1000;
  }
  receive(message: LiveServerMessage) {
    this.on.message(JSON.stringify(message));
  }
  serverCloses(code: number) {
    this.on.closed(code);
  }
}

function event(id: number, type: AccountEvent['type'], extra: Partial<AccountEvent> = {}): AccountEvent {
  return { id, type, at: '2026-10-03T10:00:00.000Z', ...extra };
}

let sockets: FakeSocket[];
let saved: number;
let deps: LiveDeps & { hostsChanged: Mock<() => void>; gone: Mock<(reason: RevokeReason) => void> };

function latest(): FakeSocket {
  const socket = sockets.at(-1);
  if (!socket) throw new Error('no socket was opened');
  return socket;
}

/** Lets the connection's queued message handling run. */
async function settle() {
  await vi.advanceTimersByTimeAsync(0);
}

async function connected(live: LiveAccount) {
  live.start();
  latest().receive({ type: 'challenge', nonce: NONCE, expiresAt: '2026-10-03T10:00:30.000Z' });
  await settle();
  latest().receive({ type: 'ready', latest: 0, heartbeatMs: 25_000 });
  await settle();
}

beforeEach(() => {
  vi.useFakeTimers();
  sockets = [];
  saved = 0;
  deps = {
    url: 'wss://api.test/v1/live',
    connect: (_url, on) => {
      const socket = new FakeSocket(on);
      sockets.push(socket);
      return socket;
    },
    key: async () => OWN,
    sign: async (nonce) => `signed:${nonce}`,
    token: async () => 'session-token',
    app: { platform: 'ios', version: '0.1.0' },
    cursor: {
      load: async () => saved,
      save: async (id) => {
        saved = id;
      },
    },
    hostsChanged: vi.fn<() => void>(),
    deviceAdded: vi.fn<(key: string) => void>(),
    gone: vi.fn<(reason: RevokeReason) => void>(),
    random: () => 0.5,
  };
});

afterEach(() => {
  vi.useRealTimers();
});

describe('the hello', () => {
  it('answers the challenge with the phone key, its signature and the Clerk token', async () => {
    const live = new LiveAccount(deps);
    live.start();
    latest().receive({ type: 'challenge', nonce: NONCE, expiresAt: '' });
    await settle();
    expect(latest().sent).toEqual([
      {
        type: 'hello',
        role: 'client',
        key: OWN,
        signature: `signed:${NONCE}`,
        token: 'session-token',
        app: { platform: 'ios', version: '0.1.0' },
      },
    ]);
  });

  it('stops without a token, since the phone has signed out', async () => {
    deps.token = async () => null;
    const live = new LiveAccount(deps);
    live.start();
    latest().receive({ type: 'challenge', nonce: NONCE, expiresAt: '' });
    await settle();
    expect(latest().sent).toEqual([]);
    expect(latest().closedWith).toBe(1000);
    await vi.advanceTimersByTimeAsync(120_000);
    expect(sockets).toHaveLength(1);
  });

  it('turns the API address into the live one', () => {
    expect(liveUrl('https://api.sikemux.com')).toBe('wss://api.sikemux.com/v1/live');
    expect(liveUrl('http://192.168.1.4:4000')).toBe('ws://192.168.1.4:4000/v1/live');
  });
});

describe('messages', () => {
  it('answers a ping with a pong', async () => {
    const live = new LiveAccount(deps);
    await connected(live);
    latest().receive({ type: 'ping' });
    await settle();
    expect(latest().sent.at(-1)).toEqual({ type: 'pong' });
  });

  it('rereads the hosts when one changes, saves the newest event and acknowledges it', async () => {
    const live = new LiveAccount(deps);
    await connected(live);
    latest().receive({
      type: 'events',
      events: [event(4, 'device.added', { key: HOST, role: 'host' }), event(5, 'device.changed', { key: OTHER_HOST, role: 'host' })],
    });
    await settle();
    expect(deps.hostsChanged).toHaveBeenCalledTimes(1);
    expect(saved).toBe(5);
    expect(latest().sent.at(-1)).toEqual({ type: 'ack', id: 5 });
    expect(deps.gone).not.toHaveBeenCalled();
  });

  it('skips events it already handled but still acknowledges them', async () => {
    saved = 9;
    const live = new LiveAccount(deps);
    await connected(live);
    latest().receive({ type: 'events', events: [event(8, 'device.revoked', { key: HOST, role: 'host', reason: 'removed' })] });
    await settle();
    expect(deps.hostsChanged).not.toHaveBeenCalled();
    expect(saved).toBe(9);
    expect(latest().sent.at(-1)).toEqual({ type: 'ack', id: 8 });
  });

  it('rereads the hosts after a reset', async () => {
    const live = new LiveAccount(deps);
    await connected(live);
    latest().receive({ type: 'reset', latest: 40 });
    await settle();
    expect(deps.hostsChanged).toHaveBeenCalledTimes(1);
  });

  it('ignores what it cannot read', async () => {
    const live = new LiveAccount(deps);
    await connected(live);
    latest().on.message('not json');
    latest().on.message(JSON.stringify({ type: 'something-new' }));
    await settle();
    expect(latest().closedWith).toBeUndefined();
  });
});

describe('leaving the account', () => {
  it('signs out when this phone is revoked, once, and stays closed', async () => {
    const live = new LiveAccount(deps);
    await connected(live);
    const socket = latest();
    socket.receive({ type: 'events', events: [event(7, 'device.revoked', { key: OWN, role: 'client', reason: 'removed' })] });
    socket.receive({ type: 'revoked', reason: 'removed' });
    await settle();
    socket.serverCloses(4403);
    expect(deps.gone).toHaveBeenCalledTimes(1);
    expect(deps.gone).toHaveBeenCalledWith('removed');
    expect(socket.sent.at(-1)).toEqual({ type: 'ack', id: 7 });
    expect(saved).toBe(7);
    live.start();
    await vi.advanceTimersByTimeAsync(120_000);
    expect(sockets).toHaveLength(1);
  });

  it('signs out when the account is deleted', async () => {
    const live = new LiveAccount(deps);
    await connected(live);
    latest().receive({ type: 'events', events: [event(3, 'account.deleted')] });
    await settle();
    expect(deps.gone).toHaveBeenCalledWith('account_deleted');
  });

  it('takes the reason from revoked', async () => {
    const live = new LiveAccount(deps);
    await connected(live);
    latest().receive({ type: 'revoked', reason: 'account_deleted' });
    await settle();
    expect(deps.gone).toHaveBeenCalledWith('account_deleted');
  });

  it('treats a bare 4403 as removed', async () => {
    const live = new LiveAccount(deps);
    await connected(live);
    latest().serverCloses(4403);
    expect(deps.gone).toHaveBeenCalledWith('removed');
  });

  it('does not sign out for another phone or a host leaving', async () => {
    const live = new LiveAccount(deps);
    await connected(live);
    latest().receive({ type: 'events', events: [event(2, 'device.revoked', { key: HOST, role: 'host', reason: 'signed_out' })] });
    await settle();
    expect(deps.gone).not.toHaveBeenCalled();
    expect(deps.hostsChanged).toHaveBeenCalledTimes(1);
  });
});

describe('reconnecting', () => {
  it('backs off exponentially, half of it random, up to a minute', () => {
    expect(reconnectDelay(0, 0)).toBe(500);
    expect(reconnectDelay(0, 1)).toBe(1000);
    expect(reconnectDelay(3, 0.5)).toBe(6000);
    expect(reconnectDelay(20, 0)).toBe(30_000);
    expect(reconnectDelay(20, 1)).toBe(60_000);
  });

  it('waits as long as a bye asks, plus up to a second', () => {
    expect(afterByeDelay(5000, 0)).toBe(5000);
    expect(afterByeDelay(5000, 1)).toBe(6000);
  });

  it('reconnects after a drop, waiting longer each time, and starts over once ready', async () => {
    const live = new LiveAccount(deps);
    live.start();
    latest().serverCloses(1006);
    await vi.advanceTimersByTimeAsync(749);
    expect(sockets).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(sockets).toHaveLength(2);
    latest().serverCloses(1006);
    await vi.advanceTimersByTimeAsync(1499);
    expect(sockets).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(1);
    expect(sockets).toHaveLength(3);
    latest().receive({ type: 'challenge', nonce: NONCE, expiresAt: '' });
    await settle();
    latest().receive({ type: 'ready', latest: 0, heartbeatMs: 25_000 });
    await settle();
    latest().serverCloses(1006);
    await vi.advanceTimersByTimeAsync(750);
    expect(sockets).toHaveLength(4);
  });

  it('honours bye before reconnecting', async () => {
    const live = new LiveAccount(deps);
    await connected(live);
    latest().receive({ type: 'bye', reconnectAfterMs: 10_000 });
    await settle();
    latest().serverCloses(1012);
    await vi.advanceTimersByTimeAsync(10_499);
    expect(sockets).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(sockets).toHaveLength(2);
  });

  it('waits longer when over the server limits', async () => {
    const live = new LiveAccount(deps);
    live.start();
    latest().serverCloses(4429);
    await vi.advanceTimersByTimeAsync(29_999);
    expect(sockets).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(sockets).toHaveLength(2);
  });

  it('gives way to a newer connection for the same phone', async () => {
    const live = new LiveAccount(deps);
    await connected(live);
    latest().serverCloses(4409);
    await vi.advanceTimersByTimeAsync(120_000);
    expect(sockets).toHaveLength(1);
  });

  it('reconnects when the server goes quiet for two heartbeats', async () => {
    const live = new LiveAccount(deps);
    await connected(live);
    await vi.advanceTimersByTimeAsync(49_999);
    expect(latest().closedWith).toBeUndefined();
    await vi.advanceTimersByTimeAsync(1);
    expect(sockets[0]?.closedWith).toBe(1000);
    await vi.advanceTimersByTimeAsync(750);
    expect(sockets).toHaveLength(2);
  });

  it('stays closed while stopped, and starts again fresh', async () => {
    const live = new LiveAccount(deps);
    await connected(live);
    live.stop();
    expect(sockets[0]?.closedWith).toBe(1000);
    sockets[0]?.serverCloses(1000);
    await vi.advanceTimersByTimeAsync(120_000);
    expect(sockets).toHaveLength(1);
    live.start();
    expect(sockets).toHaveLength(2);
  });

  it('tries again at once when nudged while waiting, and leaves a working connection alone', async () => {
    const live = new LiveAccount(deps);
    await connected(live);
    live.nudge();
    expect(sockets).toHaveLength(1);
    latest().serverCloses(1006);
    live.nudge();
    expect(sockets).toHaveLength(2);
    live.stop();
    live.nudge();
    expect(sockets).toHaveLength(2);
  });

  it('says when the server took the hello, each time it does', async () => {
    const ready = vi.fn();
    const live = new LiveAccount({ ...deps, ready });
    await connected(live);
    expect(ready).toHaveBeenCalledTimes(1);
  });
});

describe('readEvents', () => {
  it('handles events in id order whatever order they arrive in', () => {
    const read = readEvents(
      [
        event(6, 'device.revoked', { key: OWN, role: 'client', reason: 'signed_out' }),
        event(5, 'device.added', { key: HOST, role: 'host', at: new Date().toISOString() }),
      ],
      OWN,
      4,
    );
    expect(read).toEqual({ newest: 6, handled: 6, hostsChanged: true, added: [HOST], gone: 'signed_out' });
  });
});
