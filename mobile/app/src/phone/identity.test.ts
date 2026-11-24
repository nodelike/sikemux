import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const native = vi.hoisted(() => ({
  creates: [] as { resolve: (d: unknown) => void; reject: (e: unknown) => void }[],
  closed: [] as number[],
}));

vi.mock('@sikemux/native', () => ({
  Device: {
    create: vi.fn(
      () =>
        new Promise((resolve, reject) => {
          native.creates.push({ resolve, reject });
        }),
    ),
  },
  DeviceIdentity: class {
    id() {
      return 'aa'.repeat(32);
    }
  },
  newDeviceKey: () => new Uint8Array(32).buffer,
}));

vi.mock('@/network/network', () => ({
  askForRelaysAgain: vi.fn(),
  currentRelays: vi.fn(async () => [{ url: 'https://relay.example/', region: 'test', quicPort: null }]),
  relaySettings: (relays: unknown) => relays,
  updateRequired: () => null,
}));

describe('phone identity lifecycle', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.resetModules();
    native.creates.length = 0;
    native.closed.length = 0;
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('bounds goOffline on a hung Device.create and closes a late abandoned endpoint', async () => {
    const id = await import('./identity');

    const first = id.thisDevice();
    first.catch(() => {});
    await vi.waitFor(() => expect(native.creates).toHaveLength(1));

    // Hung Device.create must not wedge goOffline or block the next thisDevice() past 200ms.
    const offline = id.goOffline();
    await vi.advanceTimersByTimeAsync(200);
    await offline;

    const second = id.thisDevice();
    await vi.waitFor(() => expect(native.creates).toHaveLength(2));

    const lateDevice = { close: vi.fn(async () => native.closed.push(1)) };
    const goodDevice = { close: vi.fn(async () => native.closed.push(2)) };
    native.creates[0].resolve(lateDevice);
    native.creates[1].resolve(goodDevice);

    expect(await second).toBe(goodDevice);
    await vi.waitFor(() => expect(lateDevice.close).toHaveBeenCalledTimes(1));
    expect(goodDevice.close).not.toHaveBeenCalled();
  });

  it('bounds a slow device.close during goOffline before starting the next bind', async () => {
    const id = await import('./identity');

    const first = id.thisDevice();
    await vi.waitFor(() => expect(native.creates).toHaveLength(1));
    const slowClose = { close: vi.fn(() => new Promise<void>(() => {})) };
    native.creates[0].resolve(slowClose);
    expect(await first).toBe(slowClose);

    const offline = id.goOffline();
    const next = id.thisDevice();
    // Before 200ms, the new bind has not started yet while close is draining.
    await vi.advanceTimersByTimeAsync(100);
    expect(native.creates).toHaveLength(1);
    // At 200ms, closeWait expires and the new bind starts immediately.
    await vi.advanceTimersByTimeAsync(100);
    await offline;
    await vi.waitFor(() => expect(native.creates).toHaveLength(2));
    const fresh = { close: vi.fn(async () => {}) };
    native.creates[1].resolve(fresh);
    expect(await next).toBe(fresh);
  });
});
