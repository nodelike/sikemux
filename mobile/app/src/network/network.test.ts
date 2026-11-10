import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { clear } from '../../test/mocks/expo-file-system';

const release = vi.hoisted(() => ({ current: null as { version: string; platform: 'android' } | null }));
vi.mock('./installed', () => ({ installedRelease: () => release.current }));
vi.mock('@/account/config', () => ({ apiUrl: () => 'https://api.test' }));

const ANY = { nightly: '0.0.0', stable: '0.0.0' };
const ours = { url: 'https://relay.example/', region: 'test', quicPort: 7900 };

function network(android = ANY, relays: unknown[] = [ours]) {
  return { relays, minimumVersions: { macos: ANY, ios: ANY, android } };
}

let answer: (() => Promise<{ ok: boolean; json: () => Promise<unknown> }>) | null;
let asked: string[];

async function load() {
  vi.resetModules();
  return import('./network');
}

beforeEach(() => {
  clear();
  release.current = { version: '0.1.0-nightly.3', platform: 'android' };
  asked = [];
  answer = null;
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string) => {
      asked.push(url);
      if (!answer) throw new TypeError('Network request failed');
      return answer();
    }),
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
});

const serve = (body: unknown) => {
  answer = async () => ({ ok: true, json: async () => body });
};

describe('the relays', () => {
  it('come from the server and are kept for when it is out of reach', async () => {
    serve(network());
    const first = await load();
    expect(await first.currentRelays()).toEqual([ours]);
    expect(asked).toEqual(['https://api.test/v1/network']);

    answer = null;
    const offline = await load();
    expect(await offline.currentRelays()).toEqual([ours]);
  });

  it('fall back to our own relay, never to none', async () => {
    const { currentRelays, DEFAULT_RELAYS } = await load();
    expect(await currentRelays()).toEqual(DEFAULT_RELAYS);

    serve(network(ANY, []));
    const empty = await load();
    expect(await empty.currentRelays()).toEqual(DEFAULT_RELAYS);
  });

  it('are asked for again a while after a failure, or once the network is back, but not after a fresh answer', async () => {
    vi.useFakeTimers();
    try {
      const { currentRelays, askForRelaysAgain } = await load();
      await currentRelays();
      await currentRelays();
      expect(asked).toHaveLength(1);
      vi.advanceTimersByTime(31_000);
      await currentRelays();
      expect(asked).toHaveLength(2);
      askForRelaysAgain();
      serve(network());
      await currentRelays();
      await currentRelays();
      askForRelaysAgain();
      await currentRelays();
      expect(asked).toHaveLength(3);
    } finally {
      vi.useRealTimers();
    }
  });

  it('cross to the native client with their QUIC port', async () => {
    const { relaySettings } = await load();
    expect(relaySettings([ours, { ...ours, quicPort: null }])).toEqual([
      { url: ours.url, quicPort: 7900 },
      { url: ours.url, quicPort: undefined },
    ]);
  });
});

describe('an app older than the server allows', () => {
  it('needs an update, by its own platform and channel', async () => {
    serve(network({ nightly: '0.1.0-nightly.4', stable: '0.0.0' }));
    const { currentRelays, updateRequired } = await load();
    await currentRelays();
    expect(updateRequired()).toEqual({ current: '0.1.0-nightly.3', minimum: '0.1.0-nightly.4' });
  });

  it('is never blocked because the server could not be reached', async () => {
    const { currentRelays, updateRequired } = await load();
    await currentRelays();
    expect(updateRequired()).toBeNull();
  });

  it('is never blocked in a development build', async () => {
    release.current = null;
    serve(network({ nightly: '9.0.0', stable: '9.0.0' }));
    const { currentRelays, updateRequired } = await load();
    await currentRelays();
    expect(updateRequired()).toBeNull();
  });
});
