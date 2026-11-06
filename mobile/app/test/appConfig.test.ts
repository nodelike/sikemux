import { afterEach, describe, expect, it, vi } from 'vitest';

import appConfig from '../app.config.js';
import { expo } from '../app.json';
import { versionFromBuild } from '../src/network/versions';

function resolve(env: Record<string, string | undefined>) {
  for (const [name, value] of Object.entries(env)) vi.stubEnv(name, value);
  return appConfig({ config: structuredClone(expo) });
}

afterEach(() => {
  vi.unstubAllEnvs();
});

describe('updates', () => {
  it('asks for nightly updates when no channel is given', () => {
    const config = resolve({ APP_VARIANT: 'production', SIKEMUX_MOBILE_CHANNEL: undefined });
    expect(config.updates.requestHeaders).toEqual({ 'expo-channel-name': 'nightly' });
  });

  it('treats an empty channel as no channel', () => {
    const config = resolve({ APP_VARIANT: 'production', SIKEMUX_MOBILE_CHANNEL: '' });
    expect(config.updates.requestHeaders['expo-channel-name']).toBe('nightly');
  });

  it('asks for stable updates on a stable build', () => {
    const config = resolve({ APP_VARIANT: 'production', SIKEMUX_MOBILE_CHANNEL: 'stable' });
    expect(config.updates.requestHeaders['expo-channel-name']).toBe('stable');
  });

  it('refuses a channel that does not exist', () => {
    expect(() => resolve({ APP_VARIANT: 'production', SIKEMUX_MOBILE_CHANNEL: 'beta' })).toThrow('beta is not a release channel');
  });

  it('is off in dev builds, whatever the channel', () => {
    const config = resolve({ APP_VARIANT: undefined, SIKEMUX_MOBILE_CHANNEL: 'beta' });
    expect(config.updates).toEqual({ enabled: false });
    expect(config.runtimeVersion).toBeUndefined();
  });
});

describe('build numbers', () => {
  it('are the same on Android and iOS, and read back as the release', () => {
    const nightly = `${expo.version}-nightly.3`;
    const config = resolve({ APP_VARIANT: 'production', SIKEMUX_MOBILE_VERSION: nightly });
    expect(config.ios.buildNumber).toBe(String(config.android.versionCode));
    expect(versionFromBuild(config.ios.buildNumber)).toBe(nightly);
    const stable = resolve({ APP_VARIANT: 'production', SIKEMUX_MOBILE_VERSION: expo.version });
    expect(versionFromBuild(stable.ios.buildNumber)).toBe(expo.version);
  });
});

describe('release builds', () => {
  it("keep expo-gl's classes, which its C++ and Expo's view registry look up by name", () => {
    const buildProperties = expo.plugins.find((plugin) => Array.isArray(plugin) && plugin[0] === 'expo-build-properties');
    const rules = (buildProperties?.[1] as { android: { extraProguardRules: string } }).android.extraProguardRules;
    expect(rules.split('\n')).toContain('-keep class expo.modules.gl.** { *; }');
  });
});
