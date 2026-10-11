import { beforeEach, describe, expect, it, vi } from 'vitest';

import { File, clear as clearDisk } from '../../test/mocks/expo-file-system';

type Paired = typeof import('./paired');
let paired: Paired;

beforeEach(async () => {
  vi.resetModules();
  clearDisk();
  paired = await import('./paired');
});

describe('the paired hosts', () => {
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

  it('does not bring back a host forgotten while an update was on its way', async () => {
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

  it('starts with no hosts, and says so, when the saved list is damaged', async () => {
    new File('file:///document/paired-devices.json').write('[{"core": "a", "acc');
    expect(await paired.pairedDevices()).toEqual([]);
    expect(paired.pairedListDamaged()).toBe(true);
    await paired.rememberDevice({ core: 'b', access: 'full', pairedAt: 2 });
    vi.resetModules();
    const relaunched: Paired = await import('./paired');
    expect(await relaunched.pairedDevices()).toEqual([{ core: 'b', access: 'full', pairedAt: 2 }]);
    expect(relaunched.pairedListDamaged()).toBe(false);
  });

  it('reads the new copy when a write stopped before it was moved into place', async () => {
    new File('file:///document/paired-devices.json.next').write('[{"core":"a","access":"full","pairedAt":1}]');
    expect(await paired.pairedDevices()).toEqual([{ core: 'a', access: 'full', pairedAt: 1 }]);
  });

  it('reads a damaged list as fresh when read again, keeping hosts paired since and the damaged copy', async () => {
    new File('file:///document/paired-devices.json').write('not json');
    expect(await paired.pairedDevices()).toEqual([]);
    await paired.rememberDevice({ core: 'b', access: 'full', pairedAt: 2 });
    await paired.readAgain();
    expect(paired.pairedListDamaged()).toBe(false);
    expect(await paired.pairedDevices()).toEqual([{ core: 'b', access: 'full', pairedAt: 2 }]);
    expect(new File('file:///document/paired-devices.json.damaged').exists).toBe(true);
  });

  it('leaves the file alone when a change changes nothing', async () => {
    await paired.rememberDevice({ core: 'a', access: 'full', pairedAt: 1, name: 'Studio' });
    const file = new File('file:///document/paired-devices.json');
    file.write('[{"core":"a","access":"full","pairedAt":1,"name":"Studio"}]');
    await paired.updateDevice('a', { name: 'Studio' });
    expect(await file.text()).toBe('[{"core":"a","access":"full","pairedAt":1,"name":"Studio"}]');
  });
});
