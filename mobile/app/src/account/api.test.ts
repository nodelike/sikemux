import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { errorCode, explain } from '@protocol/account';

import { accountHosts, ApiProblem, clearPushToken, deleteAccount, joinTicket, registerPhone, removePhone, setPushToken } from './api';

const identity = vi.hoisted(() => ({
  deviceIdentity: vi.fn(async () => ({
    id: () => 'ab'.repeat(32),
    signRegistration: vi.fn((nonce: string, userId: string) => `signed:${nonce}:${userId}`),
    signPush: vi.fn((nonce: string, tokenSha256: string) => `pushed:${nonce}:${tokenSha256}`),
  })),
}));
vi.mock('@/device/identity', () => identity);
vi.mock('@/device/name', () => ({ phoneName: () => 'Pixel 9' }));
vi.mock('./config', () => ({ apiUrl: () => 'https://api.test' }));

type Call = { url: string; method: string; body?: unknown; authorization?: string };
let calls: Call[];
let answers: { status: number; body: unknown }[];

beforeEach(() => {
  calls = [];
  answers = [];
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init: { method: string; headers: Record<string, string>; body?: string }) => {
      calls.push({
        url,
        method: init.method,
        authorization: init.headers.authorization,
        ...(init.body ? { body: JSON.parse(init.body) } : {}),
      });
      const answer = answers.shift() ?? { status: 500, body: {} };
      return { ok: answer.status < 400, status: answer.status, json: async () => answer.body };
    }),
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
});

const token = async () => 'session-token';

describe('registerPhone', () => {
  it('signs the challenge the server gave and registers this phone as a client', async () => {
    answers.push({ status: 200, body: { nonce: 'n'.repeat(64), expiresAt: '2026-10-03T10:02:00.000Z' } });
    answers.push({ status: 201, body: { key: 'ab'.repeat(32), role: 'client' } });
    await registerPhone(token, 'user_2abc');
    expect(calls[0]).toMatchObject({ url: 'https://api.test/v1/devices/challenge', method: 'POST', authorization: 'Bearer session-token' });
    expect(calls[1]).toMatchObject({
      url: 'https://api.test/v1/devices',
      method: 'POST',
      body: {
        key: 'ab'.repeat(32),
        role: 'client',
        name: 'Pixel 9',
        nonce: 'n'.repeat(64),
        signature: `signed:${'n'.repeat(64)}:user_2abc`,
      },
    });
    expect(calls[1]?.body).not.toHaveProperty('channel');
  });

  it('passes on what the server said when it refuses', async () => {
    answers.push({ status: 200, body: { nonce: 'n'.repeat(64), expiresAt: '' } });
    answers.push({
      status: 409,
      body: { error: { code: 'conflict', message: 'This device is registered to another account.', requestId: 'r' } },
    });
    await expect(registerPhone(token, 'user_2abc')).rejects.toThrow('This device is registered to another account.');
  });

  it('asks to sign in again without a token', async () => {
    await expect(registerPhone(async () => null, 'user_2abc')).rejects.toBeInstanceOf(ApiProblem);
    expect(calls).toEqual([]);
  });
});

describe('removePhone', () => {
  it('deletes this phone by its key', async () => {
    answers.push({ status: 204, body: null });
    await removePhone(token);
    expect(calls[0]).toMatchObject({ url: `https://api.test/v1/devices/${'ab'.repeat(32)}`, method: 'DELETE' });
  });

  it('counts a phone the account no longer has as already off it', async () => {
    answers.push({ status: 404, body: { error: { code: 'not_found', message: 'None of your devices has that key.', requestId: 'r' } } });
    await expect(removePhone(token)).resolves.toBeUndefined();
  });

  it('reports a server out of reach, so signing out can ask first', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => {
        throw new TypeError('Network request failed');
      }),
    );
    const failure = await removePhone(token).catch((error: unknown) => error);
    expect(failure).toBeInstanceOf(ApiProblem);
    expect((failure as ApiProblem).unreachable).toBe(true);
  });

  it('reports a server failing on its side as out of reach too', async () => {
    answers.push({ status: 503, body: {} });
    const failure = await removePhone(token).catch((error: unknown) => error);
    expect((failure as ApiProblem).unreachable).toBe(true);
  });

  it('does not count a refused sign-in as the phone being off the account', async () => {
    answers.push({ status: 401, body: { error: { code: 'unauthorized', message: 'Sign in again.', requestId: 'r' } } });
    const failure = await removePhone(token).catch((error: unknown) => error);
    expect((failure as ApiProblem).status).toBe(401);
  });

  it('reports Clerk failing to hand over a token as out of reach', async () => {
    const failure = await removePhone(async () => {
      throw new Error('network');
    }).catch((error: unknown) => error);
    expect(failure).toBeInstanceOf(ApiProblem);
    expect((failure as ApiProblem).unreachable).toBe(true);
    expect(calls).toEqual([]);
  });
});

describe('setPushToken', () => {
  it('signs the challenge with the token hash and sends the FCM token for this build', async () => {
    answers.push({ status: 200, body: { nonce: 'n'.repeat(64), expiresAt: '' } });
    answers.push({ status: 200, body: { enabled: true, updatedAt: '2026-10-03T10:00:00.000Z' } });
    await setPushToken(token, { token: 'fcm-token', tokenSha256: 'h'.repeat(64), app: 'dev' });
    expect(calls[0]).toMatchObject({ url: 'https://api.test/v1/devices/challenge', method: 'POST' });
    expect(calls[1]).toMatchObject({
      url: `https://api.test/v1/devices/${'ab'.repeat(32)}/push`,
      method: 'PUT',
      body: {
        platform: 'fcm',
        token: 'fcm-token',
        app: 'dev',
        nonce: 'n'.repeat(64),
        signature: `pushed:${'n'.repeat(64)}:${'h'.repeat(64)}`,
      },
    });
    expect(calls[1]?.body).not.toHaveProperty('apnsEnvironment');
  });
});

describe('clearPushToken', () => {
  it("deletes this phone's token, and counts a phone the account no longer has as done", async () => {
    answers.push({ status: 204, body: null });
    await clearPushToken(token);
    expect(calls[0]).toMatchObject({ url: `https://api.test/v1/devices/${'ab'.repeat(32)}/push`, method: 'DELETE' });
    answers.push({ status: 404, body: { error: { code: 'not_found', message: 'None of your devices has that key.', requestId: 'r' } } });
    await expect(clearPushToken(token)).resolves.toBeUndefined();
  });

  it('reports a server out of reach, so signing out can ask first', async () => {
    answers.push({ status: 503, body: {} });
    const failure = await clearPushToken(token).catch((error: unknown) => error);
    expect((failure as ApiProblem).unreachable).toBe(true);
  });
});

describe('deleteAccount', () => {
  it('deletes the account', async () => {
    answers.push({ status: 202, body: { status: 'deleted', requestedAt: '2026-10-03T10:00:00.000Z' } });
    await expect(deleteAccount(token)).resolves.toEqual({ status: 'deleted', requestedAt: '2026-10-03T10:00:00.000Z' });
    expect(calls[0]).toMatchObject({ url: 'https://api.test/v1/account', method: 'DELETE', authorization: 'Bearer session-token' });
    expect(calls[0]?.body).toBeUndefined();
  });

  it("sends Apple's code for the server to revoke", async () => {
    answers.push({ status: 202, body: { status: 'deleted', requestedAt: '2026-10-03T10:00:00.000Z' } });
    await deleteAccount(token, 'apple-code');
    expect(calls[0]).toMatchObject({ method: 'DELETE', body: { appleAuthorizationCode: 'apple-code' } });
  });

  it('asks the person to prove it is them again when the server says reverify', async () => {
    answers.push({ status: 403, body: { error: { code: 'reverify_required', message: 'reverify', requestId: 'r' } } });
    await expect(deleteAccount(token)).rejects.toMatchObject({ code: 'reverify_required' });
  });

  it('passes on any other refusal', async () => {
    answers.push({ status: 403, body: { error: { code: 'forbidden', message: 'Delete the account in the phone app.', requestId: 'r' } } });
    await expect(deleteAccount(token)).rejects.toThrow('Delete the account in the phone app.');
  });
});

describe('accountHosts', () => {
  it('lists the hosts on the account', async () => {
    answers.push({ status: 200, body: { devices: [{ key: 'cd'.repeat(32), role: 'host', name: 'Studio' }] } });
    const hosts = await accountHosts(token);
    expect(calls[0]?.url).toBe('https://api.test/v1/devices?role=host');
    expect(hosts.map((host) => host.name)).toEqual(['Studio']);
  });
});

describe('joinTicket', () => {
  const ticket = {
    v: 1,
    keyId: 'prod-1',
    account: 'user_2abc',
    host: 'cd'.repeat(32),
    phone: 'ab'.repeat(32),
    issuedAt: 1791000000,
    expiresAt: 1791000600,
    signature: 'ef'.repeat(64),
  };

  it("asks for a ticket in this phone's name for the host it means to join", async () => {
    answers.push({ status: 200, body: ticket });
    await expect(joinTicket(token, 'cd'.repeat(32))).resolves.toEqual(ticket);
    expect(calls[0]).toEqual({
      url: `https://api.test/v1/devices/${'ab'.repeat(32)}/join`,
      method: 'POST',
      authorization: 'Bearer session-token',
      body: { host: 'cd'.repeat(32) },
    });
  });

  it('keeps the status of a refusal, so a host gone from the account reads differently from a busy server', async () => {
    answers.push({ status: 404, body: { error: { code: 'not_found', message: 'None of your hosts has that key.', requestId: 'r' } } });
    const gone = await joinTicket(token, 'cd'.repeat(32)).catch((error: unknown) => error);
    expect(gone).toBeInstanceOf(ApiProblem);
    expect((gone as ApiProblem).status).toBe(404);
    answers.push({ status: 429, body: { error: { code: 'rate_limited', message: 'Too many tickets.', requestId: 'r' } } });
    expect(((await joinTicket(token, 'cd'.repeat(32)).catch((error: unknown) => error)) as ApiProblem).status).toBe(429);
  });
});

describe('Clerk errors', () => {
  it('reads the code and the words from either shape Clerk reports', () => {
    const listed = { errors: [{ code: 'form_identifier_not_found', longMessage: "Couldn't find your account." }] };
    expect(errorCode(listed)).toBe('form_identifier_not_found');
    expect(explain(listed)).toBe("Couldn't find your account.");
    expect(errorCode({ code: 'form_password_incorrect', message: 'Password is incorrect.' })).toBe('form_password_incorrect');
    expect(explain(undefined)).toBe('Something went wrong; try again.');
  });
});
