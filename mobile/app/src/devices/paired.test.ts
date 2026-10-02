import { beforeEach, describe, expect, it, vi } from 'vitest';

import { clear as clearDisk } from '../../test/mocks/expo-file-system';

type Paired = typeof import('./paired');
let paired: Paired;

beforeEach(async () => {
  vi.resetModules();
  clearDisk();
  paired = await import('./paired');
});

describe('the paired Macs', () => {
  it('keeps every change made at once', async () => {
    await paired.rememberDevice({ core: 'a', access: 'full', pairedAt: 1 });
    await Promise.all([
      paired.updateDevice('a', { name: 'Studio' }),
      paired.updateDevice('a', { project: 'p' }),
      paired.rememberDevice({ core: 'b', access: 'watch', pairedAt: 2 }),
    ]);
    expect(await paired.pairedDevices()).toEqual([
      { core: 'a', access: 'full', pairedAt: 1, name: 'Studio', project: 'p' },
      { core: 'b', access: 'watch', pairedAt: 2 },
    ]);
  });

  it('does not bring back a Mac forgotten while an update was on its way', async () => {
    await paired.rememberDevice({ core: 'a', access: 'full', pairedAt: 1 });
    await Promise.all([paired.forgetDevice('a'), paired.updateDevice('a', { name: 'late' })]);
    expect(await paired.pairedDevices()).toEqual([]);
  });

  it('reads back what an earlier launch saved', async () => {
    await paired.rememberDevice({ core: 'a', access: 'full', pairedAt: 1 });
    vi.resetModules();
    const relaunched: Paired = await import('./paired');
    expect(await relaunched.pairedDevices()).toEqual([{ core: 'a', access: 'full', pairedAt: 1 }]);
  });
});
