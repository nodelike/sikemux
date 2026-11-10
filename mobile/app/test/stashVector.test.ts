import { describe, expect, it } from 'vitest';

import vector from '../../../server/protocol/vectors/stash.json';
import { shareExtensionSource } from './stashVector.mjs';

describe('the phone\'s stash uploads', () => {
  it('sends the stash upload the vector says', () => {
    const upload = shareExtensionSource('Shared/Stash/StashRules.swift');
    for (const header of Object.values(vector.headers)) expect(upload).toContain(`"${header}"`);
  });

  it('keeps to the limits the vector says', () => {
    const model = shareExtensionSource('Shared/Stash/StashRules.swift');
    const bytes = /mostBytes: Int64 = (\d+) \* 1024 \* 1024/.exec(model)?.[1];
    expect(Number(bytes) * 1024 * 1024).toBe(vector.maxFileBytes);
    expect(model).toContain(`mostText = ${vector.maxText}`);
  });
});
