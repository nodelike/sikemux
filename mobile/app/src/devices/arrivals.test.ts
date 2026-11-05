import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Device } from '@sikemux/protocol';

import { File, clear as clearDisk } from '../../test/mocks/expo-file-system';

function host(name: string, key: string): Device {
  return { key: key.repeat(64), role: 'host', name, platform: 'macos', createdAt: '2026-10-05T10:00:00Z', lastSeenAt: null };
}

const STUDIO = host('Studio', 'a');
const LAPTOP = host('MacBook Pro', 'b');
const MINI = host('Mac mini', 'c');

let hostsArrived: typeof import('./arrivals').hostsArrived;

beforeEach(async () => {
  vi.resetModules();
  clearDisk();
  ({ hostsArrived } = await import('./arrivals'));
});

describe('hosts arriving on the account', () => {
  it('only remembers the hosts an account already had the first time it looks', async () => {
    expect(await hostsArrived('user_1', [STUDIO])).toEqual([]);
    expect(await hostsArrived('user_1', [STUDIO, LAPTOP])).toEqual([LAPTOP]);
  });

  it('names a host as new once, so one that turned the phone down does not ask again', async () => {
    await hostsArrived('user_1', []);
    expect(await hostsArrived('user_1', [LAPTOP])).toEqual([LAPTOP]);
    expect(await hostsArrived('user_1', [LAPTOP])).toEqual([]);
    expect(await hostsArrived('user_1', [])).toEqual([]);
    expect(await hostsArrived('user_1', [LAPTOP])).toEqual([]);
  });

  it('remembers across a relaunch, so a host that signed in while the app was closed still counts as new', async () => {
    await hostsArrived('user_1', [STUDIO]);
    vi.resetModules();
    ({ hostsArrived } = await import('./arrivals'));
    expect(await hostsArrived('user_1', [STUDIO, MINI])).toEqual([MINI]);
  });

  it('starts over for another account', async () => {
    await hostsArrived('user_1', []);
    expect(await hostsArrived('user_2', [LAPTOP])).toEqual([]);
    expect(await hostsArrived('user_2', [LAPTOP, MINI])).toEqual([MINI]);
  });

  it('answers looks one after another, so two at once never both claim a host', async () => {
    await hostsArrived('user_1', []);
    const [first, second] = await Promise.all([hostsArrived('user_1', [LAPTOP]), hostsArrived('user_1', [LAPTOP])]);
    expect([first, second]).toEqual([[LAPTOP], []]);
  });

  it('takes a damaged list as a first look rather than stopping', async () => {
    new File('file:///document/seen-hosts.json').write('{"account":');
    expect(await hostsArrived('user_1', [STUDIO])).toEqual([]);
    expect(await hostsArrived('user_1', [STUDIO, MINI])).toEqual([MINI]);
  });
});
