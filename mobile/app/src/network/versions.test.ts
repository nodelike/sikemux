import { describe, expect, it } from 'vitest';

import { compareVersions, tooOld, versionFromBuild } from './versions';

describe('the installed version', () => {
  it('reads back from the version code', () => {
    expect(versionFromBuild('500003')).toBe('0.5.0-nightly.3');
    expect(versionFromBuild('500999')).toBe('0.5.0');
    expect(versionFromBuild('10201000')).toBe('1.2.1-nightly.0');
    expect(versionFromBuild('999')).toBeNull();
    expect(versionFromBuild('1.0')).toBeNull();
    expect(versionFromBuild(null)).toBeNull();
  });
});

describe('the oldest version allowed', () => {
  const minimum = { nightly: '0.5.0-nightly.4', stable: '0.4.2' };

  it('compares within the installed version’s channel', () => {
    expect(tooOld('0.5.0-nightly.3', minimum)).toBe('0.5.0-nightly.4');
    expect(tooOld('0.5.0-nightly.10', minimum)).toBeNull();
    expect(tooOld('0.4.1', minimum)).toBe('0.4.2');
    expect(tooOld('0.4.2', minimum)).toBeNull();
  });

  it('lets every version through at 0.0.0 and never blocks on a version it cannot read', () => {
    expect(tooOld('0.0.1-nightly.1', { nightly: '0.0.0', stable: '0.0.0' })).toBeNull();
    expect(tooOld('0.5.0-nightly.1', { nightly: 'soon', stable: '0.0.0' })).toBeNull();
  });

  it('orders prereleases below their release', () => {
    expect(compareVersions('0.5.0-nightly.9', '0.5.0')).toBeLessThan(0);
    expect(compareVersions('0.5.0', '0.5.0-nightly.9')).toBeGreaterThan(0);
    expect(compareVersions('0.5.0-nightly.2', '0.5.0-nightly.10')).toBeLessThan(0);
    expect(compareVersions('1.0.0', '0.9.9')).toBeGreaterThan(0);
  });
});
