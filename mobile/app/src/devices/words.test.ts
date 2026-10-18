import { describe, expect, it } from 'vitest';

import { folder } from '@/devices/words';

describe('folder', () => {
  it('ignores a trailing slash', () => {
    expect(folder('/Users/me/project/')).toBe('project');
  });

  it('keeps the root as it is', () => {
    expect(folder('/')).toBe('/');
  });
});
