import { describe, expect, it } from 'vitest';

import vector from '../../../server/protocol/vectors/stash.json';
import { shareExtensionSource } from './stashVector.mjs';

describe('the share extension', () => {
  it('sends the stash upload the vector says', () => {
    const upload = shareExtensionSource('Share/StashUpload.swift');
    for (const header of [vector.headers.name, vector.headers.note, vector.headers.target]) expect(upload).toContain(`"${header}"`);
  });

  it('keeps to the limits the vector says', () => {
    const model = shareExtensionSource('Share/ShareModel.swift');
    const bytes = /mostBytes: Int64 = (\d+) \* 1024 \* 1024/.exec(model)?.[1];
    expect(Number(bytes) * 1024 * 1024).toBe(vector.maxFileBytes);
    expect(model).toContain(`mostText = ${vector.maxText}`);
  });
});
