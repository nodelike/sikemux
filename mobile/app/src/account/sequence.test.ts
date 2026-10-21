import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from 'vitest';

import { ApiProblem } from './api';
import { AccountSequence, type AccountStatus, type SequenceDeps } from './sequence';

vi.mock('@/device/identity', () => ({ deviceIdentity: async () => ({}) }));
vi.mock('./config', () => ({ apiUrl: () => 'https://api.test' }));

type Pending = { resolve(): void; reject(error: unknown): void };

let registrations: Pending[];
let statuses: AccountStatus[];
let deps: SequenceDeps & {
  syncPush: Mock<() => Promise<void>>;
  hostsStale: Mock<() => void>;
  live: { start: Mock<() => void>; stop: Mock<() => void>; nudge: Mock<() => void> };
};

function registration(): Pending {
  const next = registrations.shift();
  if (!next) throw new Error('nothing is registering');
  return next;
}

async function settle() {
  await vi.advanceTimersByTimeAsync(0);
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.spyOn(console, 'warn').mockImplementation(() => {});
  registrations = [];
  statuses = [];
  deps = {
    register: () => new Promise<void>((resolve, reject) => registrations.push({ resolve, reject })),
    syncPush: vi.fn(async () => {}),
    live: { start: vi.fn<() => void>(), stop: vi.fn<() => void>(), nudge: vi.fn<() => void>() },
    hostsStale: vi.fn<() => void>(),
    status: (next) => statuses.push(next),
    random: () => 0,
  };
});

afterEach(() => {
  vi.useRealTimers();
});

describe('AccountSequence', () => {
  it('opens the live connection and sends the token only once the phone is on the account', async () => {
    const sequence = new AccountSequence(deps);
    sequence.start(true);
    expect(statuses).toEqual([{ step: 'registering' }]);
    expect(deps.live.start).not.toHaveBeenCalled();
    expect(deps.syncPush).not.toHaveBeenCalled();
    registration().resolve();
    await settle();
    expect(statuses.at(-1)).toEqual({ step: 'registered' });
    expect(deps.syncPush).toHaveBeenCalledTimes(1);
    expect(deps.live.start).toHaveBeenCalledTimes(1);
  });

  it('never opens the live connection when registering fails, so an old removal cannot sign the phone out', async () => {
    const sequence = new AccountSequence(deps);
    sequence.start(true);
    registration().reject(new ApiProblem('This device is registered to another account.', 409));
    await settle();
    expect(statuses.at(-1)).toEqual({ step: 'failed', problem: 'This device is registered to another account.', retrying: false });
    await vi.advanceTimersByTimeAsync(120_000);
    expect(registrations).toEqual([]);
    expect(deps.live.start).not.toHaveBeenCalled();
  });

  it('does nothing more once signed out while registering', async () => {
    const sequence = new AccountSequence(deps);
    sequence.start(true);
    sequence.stop();
    registration().resolve();
    await settle();
    expect(deps.live.start).not.toHaveBeenCalled();
    expect(deps.syncPush).not.toHaveBeenCalled();
  });

  it('tries again with backoff while the server is out of reach and the app is in front', async () => {
    const sequence = new AccountSequence(deps);
    sequence.start(true);
    registration().reject(new ApiProblem("Can't reach Sikemux."));
    await settle();
    expect(statuses.at(-1)).toMatchObject({ step: 'failed', retrying: true });
    await vi.advanceTimersByTimeAsync(499);
    expect(registrations).toEqual([]);
    await vi.advanceTimersByTimeAsync(1);
    registration().resolve();
    await settle();
    expect(deps.live.start).toHaveBeenCalledTimes(1);
  });

  it('treats a busy server as worth trying again', async () => {
    const sequence = new AccountSequence(deps);
    sequence.start(true);
    registration().reject(new ApiProblem('Too many requests.', 429));
    await settle();
    expect(statuses.at(-1)).toMatchObject({ retrying: true });
  });

  it('waits in the background, and tries again at once on returning to the front', async () => {
    const sequence = new AccountSequence(deps);
    sequence.start(false);
    registration().reject(new ApiProblem("Can't reach Sikemux."));
    await settle();
    await vi.advanceTimersByTimeAsync(120_000);
    expect(registrations).toEqual([]);
    sequence.foreground();
    registration().resolve();
    await settle();
    expect(deps.live.start).toHaveBeenCalledTimes(1);
  });

  it('tries again at once when the network comes back, even after a refusal', async () => {
    const sequence = new AccountSequence(deps);
    sequence.start(true);
    registration().reject(new ApiProblem('Sign in again.', 401));
    await settle();
    sequence.online();
    expect(registrations).toHaveLength(1);
  });

  it('starts the live connection after registering in the background only when the app comes to the front', async () => {
    const sequence = new AccountSequence(deps);
    sequence.start(false);
    registration().resolve();
    await settle();
    expect(deps.live.start).not.toHaveBeenCalled();
    sequence.foreground();
    expect(deps.live.start).toHaveBeenCalledTimes(1);
    expect(deps.syncPush).toHaveBeenCalledTimes(2);
    sequence.background();
    expect(deps.live.stop).toHaveBeenCalledTimes(1);
  });

  it('once registered, a returning network reconnects at once and reads the hosts again', async () => {
    const sequence = new AccountSequence(deps);
    sequence.start(true);
    registration().resolve();
    await settle();
    sequence.online();
    expect(deps.live.nudge).toHaveBeenCalledTimes(1);
    expect(deps.hostsStale).toHaveBeenCalledTimes(1);
    expect(registrations).toEqual([]);
  });

  it('sends a renewed notification token only from a phone on the account', async () => {
    const sequence = new AccountSequence(deps);
    sequence.start(true);
    sequence.pushChanged();
    expect(deps.syncPush).not.toHaveBeenCalled();
    registration().resolve();
    await settle();
    sequence.pushChanged();
    expect(deps.syncPush).toHaveBeenCalledTimes(2);
  });

  it('keeps going when the notification token cannot be sent', async () => {
    deps.syncPush.mockRejectedValue(new Error('offline'));
    const sequence = new AccountSequence(deps);
    sequence.start(true);
    registration().resolve();
    await settle();
    expect(deps.live.start).toHaveBeenCalledTimes(1);
  });
});
