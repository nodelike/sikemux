import { describe, expect, it } from 'vitest';

import { initials } from '@/account/initials';

describe('initials', () => {
  it('takes the first and last names', () => {
    expect(initials('Kishore Gunalan', 'contact@nodelike.com')).toBe('KG');
    expect(initials('  ada  augusta   lovelace ', null)).toBe('AL');
  });

  it('takes one letter from a single name', () => {
    expect(initials('Kishore', 'contact@nodelike.com')).toBe('K');
  });

  it('falls back to the email without a name', () => {
    expect(initials(null, 'contact@nodelike.com')).toBe('C');
    expect(initials('   ', 'contact@nodelike.com')).toBe('C');
  });

  it('keeps a letter outside the basic plane whole', () => {
    expect(initials('𝒜da Byron', null)).toBe('𝒜B');
  });
});
