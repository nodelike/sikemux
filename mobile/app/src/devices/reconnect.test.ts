import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { CoreEvent, type CoreListener } from '@sikemux/native';

import { clear as clearDisk } from '../../test/mocks/expo-file-system';
import NetInfo from '../../test/mocks/react-native-community-netinfo';
import { AppState } from '../../test/mocks/react-native';

class FakeConnection {
  closed = false;
  unpair = vi.fn(async () => {});
  host = vi.fn(async () => ({ name: 'Studio', model: 'Mac mini', version: '1', channel: 2 }));
  setForeground = vi.fn(async (_foreground: boolean) => {});
  clearNotifications = vi.fn(async () => {});
  call = vi.fn(async (_method: string, _params: string) => '[]');
  close() {
    this.closed = true;
  }
  isOpen() {
    return !this.closed;
  }
}

type DialCall = {
  deviceIndex: number;
  core: string;
  listener: CoreListener;
  settle: (connection: FakeConnection) => void;
  fail: (error: unknown) => void;
};

type FakeEndpoint = {
  index: number;
  closed: boolean;
  closeHang: boolean;
  close: ReturnType<typeof vi.fn>;
  connect: ReturnType<typeof vi.fn>;
};

const native = vi.hoisted(() => ({
  endpoints: [] as FakeEndpoint[],
  dials: [] as DialCall[],
  nextCloseHangs: false,
}));

vi.mock('@sikemux/native', async (actual) => {
  const real = await actual<typeof import('@sikemux/native')>();
  return {
    ...real,
    Device: {
      create: vi.fn(async () => {
        const index = native.endpoints.length;
        const closeHang = native.nextCloseHangs;
        native.nextCloseHangs = false;
        const endpoint: FakeEndpoint = {
          index,
          closed: false,
          closeHang,
          close: vi.fn(() => {
            endpoint.closed = true;
            return closeHang ? new Promise<void>(() => {}) : Promise.resolve();
          }),
          connect: vi.fn(
            (core: string, listener: CoreListener) =>
              new Promise<FakeConnection>((settle, fail) => {
                native.dials.push({ deviceIndex: index, core, listener, settle, fail });
              }),
          ),
        };
        native.endpoints.push(endpoint);
        return endpoint;
      }),
    },
    DeviceIdentity: class {
      id() {
        return 'bb'.repeat(32);
      }
    },
    newDeviceKey: () => new Uint8Array(32).buffer,
  };
});

vi.mock('@/network/network', () => ({
  askForRelaysAgain: vi.fn(),
  currentRelays: vi.fn(async () => [{ url: 'https://relay.example/', region: 'test', quicPort: null }]),
  relaySettings: (relays: unknown) => relays,
  updateRequired: () => null,
}));

vi.mock('../../modules/notify', () => ({
  notifier: {
    removeKey: vi.fn(),
    shown: vi.fn(async () => []),
    dismiss: vi.fn(),
  },
}));

const VIEW = {
  workspace: { projects: [], launchers: [], palette: {}, backdrop: { texture: false } },
  sessions: [],
  chats: [],
  attentions: [],
  recent: [],
  inbox: { items: [], working: 0, away: false, agentMessages: {}, remoteAgents: [], agents: [], projects: {} },
};

describe('background reconnect on real identity and hub', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.resetModules();
    (globalThis as { sikemuxAppState?: Set<unknown> }).sikemuxAppState?.clear();
    (globalThis as { sikemuxNetInfo?: Set<unknown> }).sikemuxNetInfo?.clear();
    AppState.emit('active');
    clearDisk();
    native.endpoints.length = 0;
    native.dials.length = 0;
    native.nextCloseHangs = false;
  });

  afterEach(() => {
    AppState.emit('active');
    vi.useRealTimers();
  });

  /**
   * Reproduces the repeat Android background/return report with a failing/sleeping host beside a healthy host
   * on real Identity + Hub code:
   * - Earlier fixes dcd1aa80d and 5ca499735 left goOffline() unawaited, dropped cached views on reload, and
   *   relied on a stale endpoint after resume/network switches.
   * - 7a1d8168f fixed the single-host case but closed the one shared endpoint whenever a sleeping host
   *   stalled or retried, cutting the healthy host beside it.
   */
  it('keeps a healthy host live beside a sleeping host, and recovers cleanly after background + network loss', async () => {
    const hub = await import('./hub');

    // First endpoint will hang its close() on background to prove real identity.goOffline() stays bounded at 200ms.
    native.nextCloseHangs = true;

    hub.watch('healthy');
    hub.watch('asleep');
    await vi.waitFor(() => expect(native.dials).toHaveLength(2));
    expect(native.endpoints).toHaveLength(1);

    const healthyDial = native.dials.find((d) => d.core === 'healthy')!;
    const healthyConn = new FakeConnection();
    healthyDial.listener.events([CoreEvent.View.new({ viewJson: JSON.stringify(VIEW) })] as never);
    healthyDial.settle(healthyConn);
    await vi.waitFor(() => expect(hub.liveOf('healthy').status).toBe('open'));

    // Sleeping host stalls past ATTEMPT_MS (25s); it must NOT close the shared endpoint or cut the healthy host.
    await vi.advanceTimersByTimeAsync(25_500);
    expect(native.endpoints[0].closed).toBe(false);
    expect(healthyConn.closed).toBe(false);
    expect(hub.liveOf('healthy').status).toBe('open');

    // Network switch while healthy is live and asleep is retrying must NOT close the shared endpoint.
    // Even if healthy's 2s probe is slow, live view activity arriving during the probe keeps it open.
    healthyConn.host.mockImplementationOnce(() => new Promise(() => {}));
    NetInfo.emit('cellular');
    await vi.advanceTimersByTimeAsync(800);
    healthyDial.listener.events([CoreEvent.View.new({ viewJson: JSON.stringify(VIEW) })] as never);
    await vi.advanceTimersByTimeAsync(1500);
    expect(native.endpoints[0].closed).toBe(false);
    expect(healthyConn.closed).toBe(false);
    expect(hub.liveOf('healthy').status).toBe('open');

    // Background -> network gone -> stay away past AWAY_MS (10s) -> network returns -> foreground.
    AppState.emit('background');
    NetInfo.emit('none');
    await vi.advanceTimersByTimeAsync(15_000);
    expect(healthyConn.closed).toBe(true);
    expect(native.endpoints[0].closed).toBe(true);

    NetInfo.emit('wifi');
    AppState.emit('active');
    // Cached view stays visible with status: 'connecting' while reconnecting (never empty "Can't reach").
    expect(hub.liveOf('healthy')).toMatchObject({ status: 'connecting', snapshot: VIEW });

    // Real identity.goOffline() capped the hung close at 200ms and binds a fresh endpoint.
    await vi.advanceTimersByTimeAsync(250);
    await vi.waitFor(() => expect(native.endpoints).toHaveLength(2));
    await vi.waitFor(() => expect(native.dials.some((d) => d.deviceIndex === 1 && d.core === 'healthy')).toBe(true));

    const freshHealthy = new FakeConnection();
    native.dials
      .filter((d) => d.deviceIndex === 1 && d.core === 'healthy')
      .at(-1)!
      .settle(freshHealthy);
    await vi.waitFor(() => expect(hub.liveOf('healthy').status).toBe('open'));
    expect(freshHealthy.closed).toBe(false);
  });
});
