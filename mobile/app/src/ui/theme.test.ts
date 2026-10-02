import { describe, expect, it } from 'vitest';

import { defaultPalette, isLight, paletteFrom, translucent } from '@/ui/theme';

describe('paletteFrom', () => {
  it('is the default palette when the Mac published none', () => {
    expect(paletteFrom(undefined)).toBe(defaultPalette);
  });

  it("takes the Mac's colours over the default", () => {
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

  it('leaves the default palette unchanged', () => {
    paletteFrom({ accent: '#00ff00' });
    expect(defaultPalette.accent).toBe('#a277ff');
  });
});

describe('translucent', () => {
  it('turns a hex colour into rgba', () => {
    expect(translucent('#a277ff', 0.5)).toBe('rgba(162, 119, 255, 0.5)');
    expect(translucent('#A277FF', 1)).toBe('rgba(162, 119, 255, 1)');
  });

  it('leaves other colour forms alone', () => {
    expect(translucent('rgba(1, 2, 3, 0.4)', 0.5)).toBe('rgba(1, 2, 3, 0.4)');
    expect(translucent('#fff', 0.5)).toBe('#fff');
  });
});

describe('isLight', () => {
  it('is false for the default dark ground', () => {
    expect(isLight(defaultPalette)).toBe(false);
  });

  it('is true for a light ground', () => {
    expect(isLight({ ...defaultPalette, ground: '#f4f4f5' })).toBe(true);
  });

  it('reads the colour part of a ground with alpha', () => {
    expect(isLight({ ...defaultPalette, ground: '#ffffff80' })).toBe(true);
  });

  it('is false for a ground it cannot read', () => {
    expect(isLight({ ...defaultPalette, ground: 'white' })).toBe(false);
  });
});
