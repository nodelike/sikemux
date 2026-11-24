import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { CoreEvent, MobileError, type CoreListener } from '@sikemux/native';

import { File, clear as clearDisk } from '../../test/mocks/expo-file-system';
import NetInfo from '../../test/mocks/react-native-community-netinfo';
import { AppState } from '../../test/mocks/react-native';

const identity = vi.hoisted(() => ({
  thisDevice: vi.fn(),
  goOffline: vi.fn(),
  refreshDevice: vi.fn(),
  notifyNetworkChange: vi.fn(),
  whileJoining: vi.fn(),
}));
vi.mock('@/phone/identity', () => identity);

const native = vi.hoisted(() => ({
  notifier: {
    removeKey: vi.fn(),
    shown: vi.fn(async (): Promise<{ tag: string; host: string }[]> => []),
    dismiss: vi.fn(),
  },
}));
vi.mock('../../modules/notify', () => native);

type Hub = typeof import('./hub');

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

function fakeDevice() {
  const calls: { core: string; listener: CoreListener; settle: (connection: FakeConnection) => void; fail: (error: unknown) => void }[] =
    [];
  const device = {
    connect: vi.fn(
      (core: string, listener: CoreListener) =>
        new Promise<FakeConnection>((settle, fail) => {
          calls.push({ core, listener, settle, fail });
        }),
    ),
  };
  return { device, calls };
}

const VIEW = {
  workspace: { projects: [], launchers: [], palette: {}, backdrop: { texture: false } },
  sessions: [],
  chats: [],
  attentions: [],
  recent: [],
  inbox: { items: [], working: 0, away: false, agentMessages: {}, remoteAgents: [], agents: [], projects: {} },
};

let hub: Hub;
let fake: ReturnType<typeof fakeDevice>;

async function opened(core = 'host') {
  hub.watch(core);
  await vi.waitFor(() => expect(fake.calls.filter((call) => call.core === core).length).toBeGreaterThan(0));
  const connection = new FakeConnection();
  fake.calls
    .filter((call) => call.core === core)
    .at(-1)!
    .settle(connection);
  await vi.waitFor(() => expect(hub.liveOf(core).status).toBe('open'));
  return connection;
}

const connects = (core = 'host') => fake.device.connect.mock.calls.filter(([dialled]) => dialled === core).length;

beforeEach(async () => {
  vi.useFakeTimers();
  vi.resetModules();
  // Hubs imported by earlier tests would otherwise still hear the app and the network move.
  (globalThis as { sikemuxAppState?: Set<unknown> }).sikemuxAppState?.clear();
  (globalThis as { sikemuxNetInfo?: Set<unknown> }).sikemuxNetInfo?.clear();
  AppState.emit('active');
  clearDisk();
  fake = fakeDevice();
  identity.thisDevice.mockResolvedValue(fake.device);
  identity.goOffline.mockResolvedValue(undefined);
  identity.refreshDevice.mockResolvedValue(undefined);
  identity.notifyNetworkChange.mockResolvedValue(undefined);
  hub = await import('./hub');
});

afterEach(() => {
  AppState.emit('active');
  vi.useRealTimers();
});

describe('the hub', () => {
  it('opens one connection however many screens watch a host at once', async () => {
    hub.watch('host');
    hub.watch('host');
    hub.watch('host');
    await vi.waitFor(() => expect(fake.device.connect).toHaveBeenCalledTimes(1));
    fake.calls[0].settle(new FakeConnection());
    await vi.waitFor(() => expect(hub.liveOf('host').status).toBe('open'));
    hub.watch('host');
    expect(fake.device.connect).toHaveBeenCalledTimes(1);
  });

  it('keeps the view the host sends before the connection is handed over', async () => {
    hub.watch('host');
    await vi.waitFor(() => expect(fake.calls).toHaveLength(1));
    fake.calls[0].listener.events([CoreEvent.View.new({ viewJson: JSON.stringify(VIEW) })] as never);
    fake.calls[0].settle(new FakeConnection());
    await vi.waitFor(() => expect(hub.liveOf('host').status).toBe('open'));
    expect(hub.liveOf('host').snapshot).toEqual(VIEW);
  });

  it('stops trying a host that forgot this phone, and tries an unreachable one again', async () => {
    hub.watch('forgot');
    hub.watch('away');
    await vi.waitFor(() => expect(fake.calls).toHaveLength(2));
    fake.calls[0].fail(MobileError.Unpaired.new());
    fake.calls[1].fail(MobileError.Connection.new({ message: 'no route' }));
    await vi.waitFor(() => expect(hub.liveOf('forgot')).toMatchObject({ status: 'closed', unpaired: true }));
    expect(hub.liveOf('away').status).toBe('connecting');
    await vi.advanceTimersByTimeAsync(20_000);
    expect(fake.device.connect.mock.calls.filter(([core]) => core === 'forgot')).toHaveLength(1);
    expect(fake.device.connect.mock.calls.filter(([core]) => core === 'away').length).toBeGreaterThan(1);
  });

  it('hands chat events over in batches, and one failing screen does not starve the others', async () => {
    hub.watch('host');
    await vi.waitFor(() => expect(fake.calls).toHaveLength(1));
    fake.calls[0].settle(new FakeConnection());
    const heard: number[] = [];
    hub.onChatEvents('host', () => {
      throw new Error('a broken screen');
    });
    hub.onChatEvents('host', (deliveries) => heard.push(deliveries.length));
    const chat = (seq: bigint) => CoreEvent.Chat.new({ agentId: 'a', seq, changesJson: '[]' });
    fake.calls[0].listener.events([chat(1n), chat(2n), chat(3n)] as never);
    expect(heard).toEqual([3]);
  });

  it('asks the host to unpair when forgotten, and never reconnects to it', async () => {
    hub.watch('host');
    await vi.waitFor(() => expect(fake.calls).toHaveLength(1));
    const connection = new FakeConnection();
    fake.calls[0].settle(connection);
    await vi.waitFor(() => expect(hub.liveOf('host').status).toBe('open'));
    await hub.forget('host');
    expect(connection.unpair).toHaveBeenCalled();
    expect(connection.closed).toBe(true);
    hub.retry('host');
    await vi.advanceTimersByTimeAsync(60_000);
    expect(fake.device.connect).toHaveBeenCalledTimes(1);
  });

  it('lets connections go once the app has been away a while, and comes back with it, telling the host which', async () => {
    hub.watch('host');
    await vi.waitFor(() => expect(fake.calls).toHaveLength(1));
    const connection = new FakeConnection();
    fake.calls[0].settle(connection);
    await vi.waitFor(() => expect(hub.liveOf('host').status).toBe('open'));

    AppState.emit('background');
    expect(connection.setForeground).toHaveBeenLastCalledWith(false);
    await vi.advanceTimersByTimeAsync(2_000);
    AppState.emit('active');
    expect(connection.closed).toBe(false);
    expect(connection.setForeground).toHaveBeenLastCalledWith(true);

    AppState.emit('background');
    await vi.advanceTimersByTimeAsync(11_000);
    expect(connection.closed).toBe(true);
    expect(identity.goOffline).toHaveBeenCalled();
    AppState.emit('active');
    await vi.waitFor(() => expect(fake.device.connect).toHaveBeenCalledTimes(2));
  });

  it('keeps every connection through a glance at Notification Center, however long ago the app was last away', async () => {
    const connection = await opened();
    AppState.emit('background');
    await vi.advanceTimersByTimeAsync(1_000);
    AppState.emit('active');
    await vi.advanceTimersByTimeAsync(60_000);
    AppState.emit('inactive');
    AppState.emit('active');
    expect(connection.closed).toBe(false);
    expect(connects()).toBe(1);
  });

  it('waits longer each time a host lets the phone in and drops it at once, and starts over once it stays up', async () => {
    vi.spyOn(Math, 'random').mockReturnValue(0.5);
    hub.watch('host');
    const flap = async () => {
      await vi.waitFor(() => expect(fake.calls).toHaveLength(connects()));
      fake.calls.at(-1)!.settle(new FakeConnection());
      await vi.waitFor(() => expect(hub.liveOf('host').status).toBe('open'));
      fake.calls.at(-1)!.listener.closed();
    };
    await flap();
    await vi.advanceTimersByTimeAsync(250);
    await flap();
    await vi.advanceTimersByTimeAsync(250);
    expect(connects()).toBe(2);
    await vi.advanceTimersByTimeAsync(500);
    expect(connects()).toBe(3);

    fake.calls.at(-1)!.settle(new FakeConnection());
    await vi.waitFor(() => expect(hub.liveOf('host').status).toBe('open'));
    await vi.advanceTimersByTimeAsync(30_000);
    fake.calls.at(-1)!.listener.closed();
    await vi.advanceTimersByTimeAsync(250);
    expect(connects()).toBe(4);
  });

  it('reads as reconnecting while a dropped connection comes back, and as unreachable only after 15 seconds', async () => {
    vi.spyOn(Math, 'random').mockReturnValue(0.5);
    const connection = await opened();
    fake.calls[0].listener.events([CoreEvent.View.new({ viewJson: JSON.stringify(VIEW) })] as never);
    await vi.advanceTimersByTimeAsync(30_000);
    fake.calls[0].listener.closed();
    expect(connection.closed).toBe(true);
    expect(hub.liveOf('host')).toEqual({ status: 'connecting', snapshot: VIEW });

    for (const wait of [250, 750, 1500, 3000, 6000]) {
      await vi.advanceTimersByTimeAsync(wait);
      await vi.waitFor(() => expect(fake.calls).toHaveLength(connects()));
      fake.calls.at(-1)!.fail(MobileError.Connection.new({ message: 'this host did not answer in time' }));
      await vi.waitFor(() => expect(hub.liveOf('host')).toEqual({ status: 'connecting', snapshot: VIEW }));
    }
    await vi.advanceTimersByTimeAsync(12_000);
    await vi.waitFor(() => expect(fake.calls).toHaveLength(connects()));
    fake.calls.at(-1)!.fail(MobileError.Connection.new({ message: 'this host did not answer in time' }));
    await vi.waitFor(() => expect(hub.liveOf('host')).toMatchObject({ status: 'closed', problem: 'this host did not answer in time' }));
  });

  it('a dropped connection that comes back never shows the host as unreachable', async () => {
    vi.spyOn(Math, 'random').mockReturnValue(0.5);
    await opened();
    await vi.advanceTimersByTimeAsync(30_000);
    fake.calls[0].listener.closed();
    await vi.advanceTimersByTimeAsync(250);
    await vi.waitFor(() => expect(connects()).toBe(2));
    fake.calls[1].fail(MobileError.Connection.new({ message: 'no route' }));
    await vi.advanceTimersByTimeAsync(750);
    await vi.waitFor(() => expect(connects()).toBe(3));
    expect(hub.liveOf('host').status).toBe('connecting');
    fake.calls[2].settle(new FakeConnection());
    await vi.waitFor(() => expect(hub.liveOf('host').status).toBe('open'));
  });

  it('shows a host it never reached as unreachable only after the grace window', async () => {
    hub.watch('host');
    await vi.waitFor(() => expect(fake.calls).toHaveLength(1));
    fake.calls[0].fail(MobileError.Connection.new({ message: 'no route' }));
    await vi.waitFor(() => expect(hub.liveOf('host')).toMatchObject({ status: 'connecting' }));
    vi.setSystemTime(Date.now() + 16_000);
    await vi.advanceTimersByTimeAsync(500);
    fake.calls.at(-1)!.fail(MobileError.Connection.new({ message: 'no route' }));
    await vi.waitFor(() => expect(hub.liveOf('host')).toMatchObject({ status: 'closed', problem: 'no route' }));
  });

  it('tries again at once when the phone gets its network back', async () => {
    NetInfo.emit('wifi', '10.0.0.2');
    hub.watch('host');
    await vi.waitFor(() => expect(fake.calls).toHaveLength(1));
    vi.setSystemTime(Date.now() + 16_000);
    fake.calls[0].fail(MobileError.Connection.new({ message: 'no route' }));
    await vi.waitFor(() => expect(hub.liveOf('host').status).toBe('closed'));
    NetInfo.emit('none');
    NetInfo.emit('wifi', '10.0.0.2');
    await vi.waitFor(() => expect(connects()).toBe(2));
    expect(hub.liveOf('host')).toMatchObject({ status: 'connecting', problem: 'no route' });
  });

  it('replaces a connection that cannot answer after the phone moves to another network', async () => {
    NetInfo.emit('wifi', '10.0.0.2');
    const connection = await opened();
    connection.host.mockImplementationOnce(() => new Promise(() => {}));
    NetInfo.emit('cellular');
    await vi.advanceTimersByTimeAsync(2_000);
    expect(connection.closed).toBe(true);
    expect(identity.refreshDevice).toHaveBeenCalled();
    expect(connects()).toBe(2);
  });

  it('comes back from a long suspension online afresh, redialling every host at once, even one stuck connecting', async () => {
    const connection = await opened('awake');
    hub.watch('stuck');
    await vi.waitFor(() => expect(connects('stuck')).toBe(1));
    AppState.emit('background');
    // A suspended app runs no timers, so only the clock moves.
    vi.setSystemTime(Date.now() + 5 * 60_000);
    AppState.emit('active');
    expect(identity.refreshDevice).toHaveBeenCalled();
    expect(connection.closed).toBe(true);
    await vi.waitFor(() => expect(connects('awake')).toBe(2));
    await vi.waitFor(() => expect(connects('stuck')).toBe(2));
  });

  it('replaces a connection that cannot answer when the app comes back from a glance', async () => {
    const connection = await opened();
    connection.host.mockImplementationOnce(() => new Promise(() => {}));
    AppState.emit('background');
    await vi.advanceTimersByTimeAsync(2_000);
    AppState.emit('active');
    await vi.advanceTimersByTimeAsync(2_000);
    expect(connection.closed).toBe(true);
    expect(identity.refreshDevice).toHaveBeenCalled();
    expect(connects()).toBe(2);
  });

  it('gives up a try that never ends, and Try again starts a fresh one at once', async () => {
    hub.watch('host');
    await vi.waitFor(() => expect(connects()).toBe(1));
    hub.retry('host');
    await vi.waitFor(() => expect(connects()).toBe(2));
    vi.setSystemTime(Date.now() + 16_000);
    await vi.advanceTimersByTimeAsync(25_000);
    expect(hub.liveOf('host')).toMatchObject({ status: 'closed', problem: 'The host did not answer in time.' });
    await vi.advanceTimersByTimeAsync(400);
    expect(connects()).toBe(3);
    const abandoned = new FakeConnection();
    fake.calls[0].settle(abandoned);
    fake.calls[2].settle(new FakeConnection());
    await vi.waitFor(() => expect(hub.liveOf('host').status).toBe('open'));
    expect(abandoned.closed).toBe(true);
  });

  it('never tears down the shared endpoint while another host is live or probing', async () => {
    NetInfo.emit('wifi', '10.0.0.2');
    const healthy = await opened('mac');
    hub.watch('asleep');
    await vi.waitFor(() => expect(connects('asleep')).toBe(1));

    // A stalled dial on the sleeping host never refreshes the shared endpoint while Mac is open.
    await vi.advanceTimersByTimeAsync(25_000);
    expect(healthy.closed).toBe(false);
    expect(identity.refreshDevice).not.toHaveBeenCalled();

    // Moving networks while the sleeping host is closed probes Mac without tearing down the endpoint first.
    NetInfo.emit('cellular');
    expect(identity.refreshDevice).not.toHaveBeenCalled();
    expect(healthy.closed).toBe(false);

    // Quick return with a slow probe on Mac and a stuck dial on asleep never cuts Mac.
    AppState.emit('background');
    await vi.advanceTimersByTimeAsync(2_000);
    AppState.emit('active');
    expect(identity.refreshDevice).not.toHaveBeenCalled();
    expect(healthy.closed).toBe(false);
  });

  it('deletes the cached view when a host turns this phone away as unpaired', async () => {
    await opened('host');
    fake.calls[0].listener.events([CoreEvent.View.new({ viewJson: JSON.stringify(VIEW) })] as never);
    await vi.advanceTimersByTimeAsync(2_500);
    expect(hub.liveOf('host').snapshot).toEqual(VIEW);
    fake.calls[0].listener.closed();
    await vi.advanceTimersByTimeAsync(400);
    fake.calls.at(-1)!.fail(MobileError.Unpaired.new());
    await vi.waitFor(() => expect(hub.liveOf('host')).toMatchObject({ status: 'closed', unpaired: true }));
    expect(hub.liveOf('host').snapshot).toBeUndefined();
  });

  it('keeps a connection that answers after the phone moves to another network', async () => {
    NetInfo.emit('wifi', '10.0.0.2');
    const connection = await opened();
    NetInfo.emit('cellular');
    await vi.advanceTimersByTimeAsync(5_000);
    expect(connection.closed).toBe(false);
    expect(connects()).toBe(1);
  });

  it('drops what a replaced connection still sends', async () => {
    await opened();
    const old = fake.calls[0].listener;
    old.closed();
    await vi.advanceTimersByTimeAsync(2_000);
    await vi.waitFor(() => expect(fake.calls).toHaveLength(2));
    const heard: bigint[] = [];
    hub.onChatEvents('host', (deliveries) => heard.push(...deliveries.map((delivery) => delivery.seq)));
    old.events([CoreEvent.Chat.new({ agentId: 'a', seq: 9n, changesJson: '[]' })] as never);
    fake.calls[1].listener.events([CoreEvent.Chat.new({ agentId: 'a', seq: 1n, changesJson: '[]' })] as never);
    expect(heard).toEqual([1n]);
  });

  it('takes only the last view of a batch, after its chat events', async () => {
    await opened();
    const order: string[] = [];
    hub.onChatEvents('host', () => order.push(`chat with view ${hub.liveOf('host').snapshot ? 'set' : 'unset'}`));
    const last = { ...VIEW, chats: [] };
    fake.calls[0].listener.events([
      CoreEvent.View.new({ viewJson: JSON.stringify(VIEW) }),
      CoreEvent.Chat.new({ agentId: 'a', seq: 1n, changesJson: '[]' }),
      CoreEvent.View.new({ viewJson: JSON.stringify(last) }),
    ] as never);
    expect(order).toEqual(['chat with view unset']);
    expect(hub.liveOf('host').snapshot).toEqual(last);
  });

  it('keeps showing a host that forgot this phone as such, until it is tried on purpose', async () => {
    hub.watch('host');
    await vi.waitFor(() => expect(fake.calls).toHaveLength(1));
    fake.calls[0].fail(MobileError.Unpaired.new());
    await vi.waitFor(() => expect(hub.liveOf('host')).toMatchObject({ status: 'closed', unpaired: true }));
    hub.watch('host');
    AppState.emit('background');
    AppState.emit('active');
    NetInfo.emit('none');
    NetInfo.emit('wifi');
    await vi.advanceTimersByTimeAsync(60_000);
    expect(connects()).toBe(1);
    hub.retry('host');
    expect(connects()).toBe(1);
    await vi.waitFor(() => expect(connects()).toBe(2));
  });

  it('withdraws the notification key and the cards when forgetting, and does not bring the host back', async () => {
    native.notifier.shown.mockResolvedValue([
      { tag: 'mine', host: 'host' },
      { tag: 'theirs', host: 'other' },
    ]);
    const connection = await opened();
    await hub.forget('host');
    expect(connection.clearNotifications).toHaveBeenCalled();
    expect(connection.clearNotifications.mock.invocationCallOrder[0]).toBeLessThan(connection.unpair.mock.invocationCallOrder[0]);
    expect(native.notifier.removeKey).toHaveBeenCalledWith('host');
    await vi.waitFor(() => expect(native.notifier.dismiss.mock.calls).toEqual([['mine']]));
    hub.watch('host');
    hub.onChatEvents('host', () => {});
    expect(hub.liveOf('host')).toMatchObject({ status: 'closed' });
    hub.rejoined('host');
    hub.watch('host');
    await vi.waitFor(() => expect(connects()).toBe(2));
  });

  it('forgets the key of a host it is not connected to', async () => {
    await hub.forget('asleep');
    expect(native.notifier.removeKey).toHaveBeenCalledWith('asleep');
  });

  it('reports a damaged list of paired hosts until it is read again', async () => {
    const paired = await import('./paired');
    new File('file:///document/paired-devices.json').write('not json');
    await hub.reloadDevices();
    await hub.reloadDevices();
    expect(paired.pairedListDamaged()).toBe(true);
    await hub.readDevicesAgain();
    expect(paired.pairedListDamaged()).toBe(false);
  });
});
