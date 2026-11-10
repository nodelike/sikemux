import { beforeEach, describe, expect, it, vi } from 'vitest';
import * as Notifications from 'expo-notifications';
import * as SecureStore from 'expo-secure-store';

import { choose } from './setting';
import { RESEND_MS, shouldSend, stopPush, syncPushToken } from './token';

const api = vi.hoisted(() => ({
  setPushToken: vi.fn(async () => ({ enabled: true, updatedAt: '2026-10-03T00:00:00.000Z' })),
  clearPushToken: vi.fn(async () => {}),
}));
vi.mock('@/account/api', () => api);

const native = vi.hoisted(() => ({
  notifier: { setPhone: vi.fn(), removeAll: vi.fn() },
}));
vi.mock('../../modules/notify', () => native);

vi.mock('@/phone/identity', () => ({ deviceIdentity: async () => ({ id: () => 'ab'.repeat(32) }) }));

const token = async () => 'session';
const TOKEN_SHA = 'c'.repeat(64);

describe('shouldSend', () => {
  const wanted = { device: 'd', tokenSha256: TOKEN_SHA, app: 'production' as const };
  const sent = { ...wanted, at: 1_000 };

  it('sends a token the server has not had, or one that changed in any way', () => {
    expect(shouldSend(null, wanted, 1_000)).toBe(true);
    expect(shouldSend(sent, { ...wanted, tokenSha256: 'other' }, 1_000)).toBe(true);
    expect(shouldSend(sent, { ...wanted, device: 'other' }, 1_000)).toBe(true);
    expect(shouldSend(sent, { ...wanted, app: 'dev' }, 1_000)).toBe(true);
  });

  it('leaves an unchanged token alone for thirty days, then sends it again', () => {
    expect(shouldSend(sent, wanted, 1_000 + RESEND_MS)).toBe(false);
    expect(shouldSend(sent, wanted, 1_001 + RESEND_MS)).toBe(true);
  });
});

describe('syncPushToken', () => {
  beforeEach(async () => {
    (SecureStore as unknown as { clear(): void }).clear();
    api.setPushToken.mockClear();
    api.clearPushToken.mockClear();
    await choose(undefined);
  });

  it('sends the token once while notifications are on and allowed, as the dev app', async () => {
    vi.spyOn(Notifications, 'getPermissionsAsync').mockResolvedValue({ granted: true } as never);
    await choose('on');
    await syncPushToken(token);
    await syncPushToken(token);
    expect(api.setPushToken).toHaveBeenCalledTimes(1);
    expect(api.setPushToken).toHaveBeenCalledWith(token, {
      token: 'fcm-token',
      tokenSha256: expect.stringMatching(/^[0-9a-f]{64}$/),
      app: 'dev',
    });
    expect(native.notifier.setPhone).toHaveBeenCalledWith('ab'.repeat(32));
  });

  it('takes the token back once the system stops allowing notifications', async () => {
    const permission = vi.spyOn(Notifications, 'getPermissionsAsync').mockResolvedValue({ granted: true } as never);
    await choose('on');
    await syncPushToken(token);
    permission.mockResolvedValue({ granted: false } as never);
    await syncPushToken(token);
    expect(api.clearPushToken).toHaveBeenCalledTimes(1);
    await syncPushToken(token);
    expect(api.clearPushToken).toHaveBeenCalledTimes(1);
  });

  it('sends nothing to a person who never turned notifications on', async () => {
    vi.spyOn(Notifications, 'getPermissionsAsync').mockResolvedValue({ granted: true } as never);
    await syncPushToken(token);
    expect(api.setPushToken).not.toHaveBeenCalled();
    expect(api.clearPushToken).not.toHaveBeenCalled();
  });

  it('lets an unreachable server stop sign-out, keeping what to take back for later', async () => {
    api.clearPushToken.mockRejectedValueOnce(new Error('offline'));
    await expect(stopPush(token)).rejects.toThrow('offline');
  });
});
