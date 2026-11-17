import { describe, expect, it, vi } from 'vitest';
import { MobileError } from '@sikemux/native';

import type { Snapshot } from '@/core/protocol';
import { AppState } from '../../test/mocks/react-native';
import { answerFromCard, answerTask, settledCards } from './cards';

const connection = vi.hoisted(() => ({ answerPermission: vi.fn(async () => {}), close: vi.fn() }));
const device = vi.hoisted(() => ({ connect: vi.fn(async () => connection) }));
const identity = vi.hoisted(() => ({
  thisDevice: async () => device,
  whileJoining: vi.fn(async <T>(work: (held: typeof device) => Promise<T>) => work(device)),
  goOffline: vi.fn(async () => {}),
}));
vi.mock('@/phone/identity', () => identity);

const HOST = 'ea'.repeat(32);

function snapshot(requests: string[]): Snapshot {
  return { attentions: requests.map((id) => ({ id })) } as unknown as Snapshot;
}

describe('settledCards', () => {
  it('picks the permission cards from this host whose request is no longer pending', () => {
    const shown = [
      { tag: 'a', host: HOST, kind: 'permission', request: 'r1' },
      { tag: 'b', host: HOST, kind: 'permission', request: 'r2' },
      { tag: 'c', host: 'other', kind: 'permission', request: 'r3' },
      { tag: 'd', host: HOST, kind: 'finished' },
    ];
    expect(settledCards(shown, HOST, snapshot(['r2']))).toEqual(['a']);
  });
});

describe('answerFromCard', () => {
  const answer = { tag: 't', host: HOST, agent: 'chat-7f3a', request: 'r1', option: 'allow', allow: true };

  it('tells a request answered elsewhere from a host it could not reach', async () => {
    connection.answerPermission.mockRejectedValueOnce(
      MobileError.Refused.new({ message: 'ACP permission request is no longer pending', kind: 'notPending' }),
    );
    expect(await answerFromCard(answer)).toBe('gone');
    device.connect.mockRejectedValueOnce(MobileError.Connection.new({ message: 'this host did not answer in time' }));
    expect(await answerFromCard(answer)).toBe('failed');
  });

  it('takes the phone off the network again after answering with the app away', async () => {
    AppState.emit('background');
    await answerTask(answer);
    expect(identity.goOffline).toHaveBeenCalled();
    AppState.emit('active');
  });
});
