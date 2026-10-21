import { beforeEach, describe, expect, it, vi } from 'vitest';
import { JoinAnswer, MobileError } from '@sikemux/native';

import { clear as clearDisk } from '../../test/mocks/expo-file-system';
import type { JoinStep } from './joining';

const device = vi.hoisted(() => ({ join: vi.fn() }));
const api = vi.hoisted(() => ({ joinTicket: vi.fn() }));
const hub = vi.hoisted(() => ({ reloadDevices: vi.fn(async () => {}), rejoined: vi.fn() }));

vi.mock('@/device/identity', () => ({ whileJoining: (work: (device: unknown) => Promise<unknown>) => work(device) }));
vi.mock('@/account/api', async (original) => ({ ...(await original<typeof import('@/account/api')>()), ...api }));
vi.mock('./hub', () => hub);
vi.mock('@/account/config', () => ({ apiUrl: () => 'https://api.test' }));

const HOST = { core: 'cd'.repeat(32), name: 'Work MacBook' };
const TICKET = { v: 1, keyId: 'prod-1', account: 'user_2abc', host: HOST.core, phone: 'ab'.repeat(32), signature: 'ef'.repeat(64) };
const token = async () => 'session-token';

let steps: JoinStep[];
let ApiProblem: typeof import('@/account/api').ApiProblem;
let JoinFailed: typeof import('./joining').JoinFailed;
let joinHost: typeof import('./joining').joinHost;
let pairedDevices: typeof import('./paired').pairedDevices;

async function failure(signal?: AbortSignal) {
  const error = await joinHost(HOST, token, (step) => steps.push(step), signal).catch((thrown: unknown) => thrown);
  expect(error).toBeInstanceOf(JoinFailed);
  return (error as InstanceType<typeof JoinFailed>).failure;
}

beforeEach(async () => {
  vi.resetModules();
  clearDisk();
  ({ ApiProblem } = await import('@/account/api'));
  ({ JoinFailed, joinHost } = await import('./joining'));
  ({ pairedDevices } = await import('./paired'));
  steps = [];
  api.joinTicket.mockResolvedValue(TICKET);
});

describe('joining a host on the account', () => {
  it('hands the host the ticket Sikemux signed and keeps the host as paired with the access it gave', async () => {
    device.join.mockResolvedValue(JoinAnswer.Allowed.new({ access: 'watch' }));
    await expect(joinHost(HOST, token, (step) => steps.push(step))).resolves.toBe('watch');
    expect(steps).toEqual(['asking', 'waiting']);
    expect(api.joinTicket).toHaveBeenCalledWith(token, HOST.core);
    expect(device.join).toHaveBeenCalledWith(HOST.core, JSON.stringify(TICKET), 'Test iPhone', 'ios', undefined);
    expect(await pairedDevices()).toEqual([expect.objectContaining({ core: HOST.core, access: 'watch', name: 'Work MacBook' })]);
    expect(hub.rejoined).toHaveBeenCalledWith(HOST.core);
    expect(hub.reloadDevices).toHaveBeenCalled();
  });

  it('says the host turned the phone down, and pairs nothing', async () => {
    device.join.mockResolvedValue(JoinAnswer.Denied.new());
    expect(await failure()).toEqual({ title: 'Work MacBook said no', detail: 'Someone at Work MacBook turned this phone down.' });
    expect(await pairedDevices()).toEqual([]);
  });

  it("says why the host refused the ticket, in the person's words where it can", async () => {
    device.join.mockResolvedValue(JoinAnswer.Refused.new({ reason: 'signed_out' }));
    expect((await failure()).detail).toBe('Work MacBook is not signed in to Sikemux. Sign it in, then try again.');
    device.join.mockResolvedValue(JoinAnswer.Refused.new({ reason: 'wrong_account' }));
    expect((await failure()).detail).toContain('different account');
    device.join.mockResolvedValue(JoinAnswer.Refused.new({ reason: 'bad_signature' }));
    expect(await failure()).toEqual({
      title: "Work MacBook couldn't let this phone in",
      detail: 'It turned down the invitation (bad_signature). Try again.',
    });
  });

  it('tells a host out of reach from Sikemux out of reach', async () => {
    device.join.mockRejectedValue(MobileError.Connection.new({ message: 'could not reach the host: timed out' }));
    expect((await failure()).title).toBe("Can't reach Work MacBook");

    api.joinTicket.mockRejectedValue(new ApiProblem("Can't reach Sikemux. Check the phone is online."));
    expect((await failure()).title).toBe("Can't reach Sikemux");
    expect(steps.at(-1)).toBe('asking');
  });

  it('says when the host has left the account or the phone asked too often', async () => {
    api.joinTicket.mockRejectedValue(new ApiProblem('None of your hosts has that key.', 404));
    expect((await failure()).title).toBe('Work MacBook is no longer on your account');
    api.joinTicket.mockRejectedValue(new ApiProblem('Too many tickets.', 429));
    expect((await failure()).title).toBe('Too many tries');
    expect(device.join).not.toHaveBeenCalled();
  });

  it('keeps nothing when the person stopped waiting before the host answered', async () => {
    const controller = new AbortController();
    device.join.mockImplementation(async () => {
      controller.abort();
      return JoinAnswer.Allowed.new({ access: 'full' });
    });
    await expect(joinHost(HOST, token, () => {}, controller.signal)).rejects.not.toBeInstanceOf(JoinFailed);
    expect(device.join).toHaveBeenCalledWith(HOST.core, JSON.stringify(TICKET), 'Test iPhone', 'ios', { signal: controller.signal });
    expect(await pairedDevices()).toEqual([]);
  });
});
