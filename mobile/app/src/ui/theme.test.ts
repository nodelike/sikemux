import { describe, expect, it } from 'vitest';

import { defaultPalette, isLight, paletteFrom, translucent } from '@/ui/theme';

describe('paletteFrom', () => {
  it("takes the host's colours over the default", () => {
    const palette = paletteFrom({ accent: '#00ff00', ground: '#ffffff' });
    expect(palette.accent).toBe('#00ff00');
    expect(palette.ground).toBe('#ffffff');
    expect(palette.ink).toBe(defaultPalette.ink);
  });

  it('ignores colours the phone does not draw, and empty ones', () => {
    const palette = paletteFrom({ notAColour: '#123456', accent: '' });
    expect(palette).not.toHaveProperty('notAColour');
    expect(palette.accent).toBe(defaultPalette.accent);
  });
});

describe('translucent', () => {
  it('turns a hex colour into rgba', () => {
    expect(translucent('#a277ff', 0.5)).toBe('rgba(162, 119, 255, 0.5)');
    expect(translucent('#A277FF', 1)).toBe('rgba(162, 119, 255, 1)');
  });
});

describe('isLight', () => {
  it('is true for a light ground', () => {
    expect(isLight({ ...defaultPalette, ground: '#f4f4f5' })).toBe(true);
  });

  it('reads the colour part of a ground with alpha', () => {
    expect(isLight({ ...defaultPalette, ground: '#ffffff80' })).toBe(true);
  });
});
