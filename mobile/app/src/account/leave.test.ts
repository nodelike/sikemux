import { beforeEach, describe, expect, it, vi } from 'vitest';

import { signOutHere } from './leave';

const hub = vi.hoisted(() => ({ forget: vi.fn(async (_core: string) => {}) }));
vi.mock('@/devices/hub', () => hub);

const paired = vi.hoisted(() => ({ pairedDevices: vi.fn(async () => [{ core: 'host-a' }, { core: 'host-b' }]) }));
vi.mock('@/devices/paired', () => paired);

const lock = vi.hoisted(() => ({ turnLockOff: vi.fn(async () => {}) }));
vi.mock('@/lock/lock', () => lock);

const setting = vi.hoisted(() => ({ choose: vi.fn(async () => {}) }));
vi.mock('@/notify/setting', () => setting);

const token = vi.hoisted(() => ({ forgetPush: vi.fn(async () => {}) }));
vi.mock('@/notify/token', () => token);

const rotate = vi.hoisted(() => ({ rotateDeviceKey: vi.fn(async () => {}) }));
vi.mock('@/phone/identity', () => rotate);

const live = vi.hoisted(() => ({ stopLive: vi.fn(), forgetCursor: vi.fn(async () => {}) }));
vi.mock('./live', () => live);

const farewell = vi.hoisted(() => ({ sayFarewell: vi.fn() }));
vi.mock('./farewell', () => farewell);

beforeEach(() => {
  vi.clearAllMocks();
});

describe('signOutHere', () => {
  it('signs out of Clerk only after asking every paired host to forget it, while the hosts can still check who asks', async () => {
    const order: string[] = [];
    hub.forget.mockImplementation(async (core) => void order.push(core));
    await signOutHere(async () => void order.push('clerk'), { confirmed: true });
    expect(order).toEqual(['host-a', 'host-b', 'clerk']);
  });

  it('keeps the device key when the server took the phone off the account', async () => {
    await signOutHere(async () => {}, { confirmed: true });
    expect(rotate.rotateDeviceKey).not.toHaveBeenCalled();
  });

  it('makes the phone a new device when the server never confirmed it left', async () => {
    await signOutHere(async () => {}, { confirmed: false });
    expect(rotate.rotateDeviceKey).toHaveBeenCalledTimes(1);
  });

  it('shares one run between two callers at once', async () => {
    const signOut = vi.fn(async () => {});
    await Promise.all([signOutHere(signOut, { confirmed: true }), signOutHere(signOut, { confirmed: true })]);
    expect(signOut).toHaveBeenCalledTimes(1);
    await signOutHere(signOut, { confirmed: true });
    expect(signOut).toHaveBeenCalledTimes(2);
  });

  it('signs out of Clerk even when forgetting fails', async () => {
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    token.forgetPush.mockRejectedValueOnce(new Error('no'));
    paired.pairedDevices.mockRejectedValueOnce(new Error('damaged'));
    rotate.rotateDeviceKey.mockRejectedValueOnce(new Error('keychain'));
    const signOut = vi.fn(async () => {});
    await signOutHere(signOut, { confirmed: false });
    expect(signOut).toHaveBeenCalledTimes(1);
  });

  it('says why the phone signed out when it was not the person here', async () => {
    await signOutHere(async () => {}, { farewell: 'removed', confirmed: true });
    expect(farewell.sayFarewell).toHaveBeenCalledWith('removed');
  });
});
