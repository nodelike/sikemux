import { beforeEach, describe, expect, it, vi } from 'vitest';
import * as Notifications from 'expo-notifications';
import * as SecureStore from 'expo-secure-store';

import { choose, notificationsChoice } from './setting';
import { turnOff, turnOn } from './switch';

const native = vi.hoisted(() => ({ notifier: { setPhone: vi.fn(), removeAll: vi.fn() } }));
vi.mock('../../modules/notify', () => native);

const hub = vi.hoisted(() => ({
  openConnections: vi.fn(() => [['host-a', { id: 'a' }] as const, ['host-b', { id: 'b' }] as const]),
  shareKeys: vi.fn(),
}));
vi.mock('@/devices/hub', () => hub);

const keys = vi.hoisted(() => ({ withdrawKey: vi.fn(async () => {}) }));
vi.mock('./keys', () => keys);

const token = vi.hoisted(() => ({
  notificationsAllowed: vi.fn(async () => true),
  stopPush: vi.fn(async () => {}),
  syncPushToken: vi.fn(async () => {}),
}));
vi.mock('./token', () => token);

const session = async () => 'session';

beforeEach(async () => {
  (SecureStore as unknown as { clear(): void }).clear();
  vi.clearAllMocks();
  await choose(undefined);
});

describe('turnOn', () => {
  it('keeps the choice but sends nothing when the system says no', async () => {
    await expect(turnOn(session)).resolves.toBe(false);
    expect(await notificationsChoice()).toBe('on');
    expect(token.syncPushToken).not.toHaveBeenCalled();
    expect(hub.shareKeys).not.toHaveBeenCalled();
  });

  it('still shares keys when the server cannot take the token yet', async () => {
    vi.spyOn(Notifications, 'requestPermissionsAsync').mockResolvedValue({ granted: true } as never);
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    token.syncPushToken.mockRejectedValueOnce(new Error('offline'));
    await expect(turnOn(session)).resolves.toBe(true);
    expect(hub.shareKeys).toHaveBeenCalledTimes(1);
  });
});

describe('turnOff', () => {
  it('stays off when the server cannot be told', async () => {
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    token.stopPush.mockRejectedValueOnce(new Error('offline'));
    await expect(turnOff(session)).resolves.toBeUndefined();
    expect(await notificationsChoice()).toBe('off');
  });
});
