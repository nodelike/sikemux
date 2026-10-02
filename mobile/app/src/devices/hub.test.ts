import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { CoreEvent, MobileError, type CoreListener } from '@sikemux/native';

import { clear as clearDisk } from '../../test/mocks/expo-file-system';
import { AppState } from '../../test/mocks/react-native';

const identity = vi.hoisted(() => ({ thisDevice: vi.fn(), goOffline: vi.fn() }));
vi.mock('@/device/identity', () => identity);

type Hub = typeof import('./hub');

class FakeConnection {
  closed = false;
  unpair = vi.fn(async () => {});
  host = vi.fn(async () => ({ name: 'Studio', model: 'Mac mini', version: '1', channel: 2 }));
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
  workspace: { projects: [], launchers: [], palette: new Map(), backdrop: { texture: false } },
  sessions: [],
  chats: [],
  attentions: [],
};

let hub: Hub;
let fake: ReturnType<typeof fakeDevice>;

beforeEach(async () => {
  vi.useFakeTimers();
  vi.resetModules();
  AppState.emit('active');
  clearDisk();
  fake = fakeDevice();
  identity.thisDevice.mockResolvedValue(fake.device);
  identity.goOffline.mockResolvedValue(undefined);
  hub = await import('./hub');
});

afterEach(() => {
  AppState.emit('active');
  vi.useRealTimers();
});

describe('the hub', () => {
  it('opens one connection however many screens watch a Mac at once', async () => {
    hub.watch('mac');
    hub.watch('mac');
    hub.watch('mac');
    await vi.waitFor(() => expect(fake.device.connect).toHaveBeenCalledTimes(1));
    fake.calls[0].settle(new FakeConnection());
    await vi.waitFor(() => expect(hub.liveOf('mac').status).toBe('open'));
    hub.watch('mac');
    expect(fake.device.connect).toHaveBeenCalledTimes(1);
  });

  it('keeps the view the Mac sends before the connection is handed over', async () => {
    hub.watch('mac');
    await vi.waitFor(() => expect(fake.calls).toHaveLength(1));
    fake.calls[0].listener.events([CoreEvent.View.new({ view: VIEW })] as never);
    fake.calls[0].settle(new FakeConnection());
    await vi.waitFor(() => expect(hub.liveOf('mac').status).toBe('open'));
    expect(hub.liveOf('mac').snapshot).toBe(VIEW);
  });

  it('stops trying a Mac that forgot this phone, and tries an unreachable one again', async () => {
    hub.watch('forgot');
    hub.watch('away');
    await vi.waitFor(() => expect(fake.calls).toHaveLength(2));
    fake.calls[0].fail(MobileError.Unpaired.new());
    fake.calls[1].fail(MobileError.Connection.new({ message: 'no route' }));
    await vi.waitFor(() => expect(hub.liveOf('away').status).toBe('closed'));
    expect(hub.liveOf('forgot')).toMatchObject({ status: 'closed', unpaired: true });
    await vi.advanceTimersByTimeAsync(60_000);
    expect(fake.device.connect.mock.calls.filter(([core]) => core === 'forgot')).toHaveLength(1);
    expect(fake.device.connect.mock.calls.filter(([core]) => core === 'away').length).toBeGreaterThan(1);
  });

  it('hands chat events over in batches, and one failing screen does not starve the others', async () => {
    hub.watch('mac');
    await vi.waitFor(() => expect(fake.calls).toHaveLength(1));
    fake.calls[0].settle(new FakeConnection());
    const heard: number[] = [];
    hub.onChatEvents('mac', () => {
      throw new Error('a broken screen');
    });
    hub.onChatEvents('mac', (deliveries) => heard.push(deliveries.length));
    const chat = (seq: bigint) => CoreEvent.Chat.new({ agentId: 'a', seq, eventJson: '{}' });
    fake.calls[0].listener.events([chat(1n), chat(2n), chat(3n)] as never);
    expect(heard).toEqual([3]);
  });

  it('asks the Mac to unpair when forgotten, and never reconnects to it', async () => {
    hub.watch('mac');
    await vi.waitFor(() => expect(fake.calls).toHaveLength(1));
    const connection = new FakeConnection();
    fake.calls[0].settle(connection);
    await vi.waitFor(() => expect(hub.liveOf('mac').status).toBe('open'));
    await hub.forget('mac');
    expect(connection.unpair).toHaveBeenCalled();
    expect(connection.closed).toBe(true);
    hub.retry('mac');
    await vi.advanceTimersByTimeAsync(60_000);
    expect(fake.device.connect).toHaveBeenCalledTimes(1);
  });

  it('lets connections go once the app has been away a while, and comes back with it', async () => {
    hub.watch('mac');
    await vi.waitFor(() => expect(fake.calls).toHaveLength(1));
    const connection = new FakeConnection();
    fake.calls[0].settle(connection);
    await vi.waitFor(() => expect(hub.liveOf('mac').status).toBe('open'));

    AppState.emit('background');
    await vi.advanceTimersByTimeAsync(2_000);
    AppState.emit('active');
    expect(connection.closed).toBe(false);

    AppState.emit('background');
    await vi.advanceTimersByTimeAsync(11_000);
    expect(connection.closed).toBe(true);
    expect(identity.goOffline).toHaveBeenCalled();
    AppState.emit('active');
    await vi.waitFor(() => expect(fake.device.connect).toHaveBeenCalledTimes(2));
  });
});
