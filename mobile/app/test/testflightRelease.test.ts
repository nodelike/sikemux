import { describe, expect, it } from 'vitest';

import { nextStep } from '../scripts/testflight-release.mjs';

describe('TestFlight releases', () => {
  it('submits a processed build for beta review, waits on one still processing, and leaves a reviewed one alone', () => {
    expect(nextStep('READY_FOR_BETA_SUBMISSION')).toBe('submit');
    expect(nextStep('PROCESSING')).toBe('wait');
    for (const state of ['WAITING_FOR_BETA_REVIEW', 'IN_BETA_REVIEW', 'BETA_APPROVED', 'IN_BETA_TESTING']) {
      expect(nextStep(state)).toBe('done');
    }
  });

  it('stops on a build TestFlight will not send to testers', () => {
    expect(() => nextStep('MISSING_EXPORT_COMPLIANCE')).toThrow(/MISSING_EXPORT_COMPLIANCE/);
    expect(() => nextStep('BETA_REJECTED')).toThrow(/BETA_REJECTED/);
  });
});
