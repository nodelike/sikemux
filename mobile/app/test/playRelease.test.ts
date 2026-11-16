import { describe, expect, it } from 'vitest';

import { FIRST_ROLLOUT, releaseNotes, track, trackRelease, widened } from '../scripts/play-release.mjs';

describe('Google Play releases', () => {
  it('sends a nightly to testers at once and a stable release to a share of production phones', () => {
    expect(track('0.5.0-nightly.3')).toBe('alpha');
    expect(trackRelease('0.5.0-nightly.3', 50003, 'notes')).toMatchObject({ status: 'completed', versionCodes: ['50003'] });
    expect(track('0.5.0')).toBe('production');
    expect(trackRelease('0.5.0', 50099, 'notes')).toMatchObject({ status: 'inProgress', userFraction: FIRST_ROLLOUT });
    expect(FIRST_ROLLOUT).toBeLessThan(1);
  });

  it('widens the release rolling out, and finishes it at every phone', () => {
    const done = { name: '0.4.0', status: 'completed', versionCodes: ['40099'] };
    const rolling = { name: '0.5.0', status: 'inProgress', userFraction: 0.1, versionCodes: ['50099'] };
    expect(widened([done, rolling], 0.5)).toEqual([done, { ...rolling, userFraction: 0.5 }]);
    expect(JSON.parse(JSON.stringify(widened([done, rolling], 1)))).toEqual([
      { name: '0.5.0', status: 'completed', versionCodes: ['50099'] },
    ]);
  });

  it('refuses to narrow a rollout, a share outside 0 to 1, or a track with nothing rolling out', () => {
    const rolling = { name: '0.5.0', status: 'inProgress', userFraction: 0.5, versionCodes: ['50099'] };
    expect(() => widened([rolling], 0.2)).toThrow(/already reaches/);
    expect(() => widened([rolling], 2)).toThrow(/between 0 and 1/);
    expect(() => widened([rolling], Number.NaN)).toThrow(/between 0 and 1/);
    expect(() => widened([{ ...rolling, status: 'completed' }], 0.8)).toThrow(/No production release/);
  });

  it('takes notes only for the version being released, in plain text', () => {
    expect(releaseNotes('0.5.0', '# 0.5.0\n\nFaster sign-in.')).toBe('Faster sign-in.');
    expect(() => releaseNotes('0.5.1', '# 0.5.0\n\nFaster sign-in.')).toThrow(/not 0.5.1/);
    expect(() => releaseNotes('0.5.0', '# 0.5.0\n\n**Faster** sign-in.')).toThrow(/plain text/);
  });
});
