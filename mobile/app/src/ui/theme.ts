import { createContext, useContext } from 'react';
import type { TextStyle } from 'react-native';

/** Aura Noir, the Mac app's default theme, with its neutral ramp mixed the same way (DESIGN.md §3). */
export const defaultPalette = {
  sunken: '#09090b',
  ground: '#0f0f13',
  raised: '#18181c',
  overlay: '#1e1e22',
  // The Mac's composer sits on its darkest surface; this keeps that step above the phone's ground.
  composer: '#17171b',
  border: '#27272b',
  borderStrong: '#3f3f43',
  rest: '#6a6a6f',

  ink: '#e7e5ef',
  secondary: '#b3b2ba',
  tertiary: '#86858c',
  inkDim: '#8b8898',
  inkFaint: '#736f80',

  active: 'rgba(231, 229, 239, 0.10)',
  selected: 'rgba(162, 119, 255, 0.14)',
  borderSelected: 'rgba(162, 119, 255, 0.42)',

  accent: '#a277ff',
  accentSoft: 'rgba(162, 119, 255, 0.10)',
  live: '#61ffca',
  warn: '#ffca85',
  danger: '#ff6767',
  cmd: '#ff6ac1',
  treeSpine: '#5f4a8a',
  treeTick: 'rgba(162, 119, 255, 0.72)',
  gitAdded: '#81b88b',
  gitModified: '#e2c08d',
  gitDeleted: '#d75f47',
  gitRenamed: '#6c8cd5',
  toolRead: '#7183c0',
  toolEdit: '#cdaf86',
  toolDelete: '#c25e4b',
  toolRun: '#d966ae',
  /** The pane grain's dots: the raised surface on a dark theme. */
  shaderDot: '#19191e',
};

export type Palette = Record<keyof typeof defaultPalette, string>;

/** The palette a Mac published, over the default for any colour it left out. */
export function paletteFrom(published: Readonly<Record<string, string>> | undefined): Palette {
  if (!published) return defaultPalette;
  const palette: Palette = { ...defaultPalette };
  for (const name of Object.keys(defaultPalette) as (keyof Palette)[]) {
    const value = published[name];
    if (value) palette[name] = value;
  }
  return palette;
}

/** A palette colour at some opacity, for surfaces that let the backdrop show through. */
export function translucent(color: string, opacity: number): string {
  const hex = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(color);
  if (!hex) return color;
  const [red, green, blue] = hex.slice(1).map((part) => parseInt(part, 16));
  return `rgba(${red}, ${green}, ${blue}, ${opacity})`;
}

/** Whether a palette's ground is light, so the status bar's text turns dark on it. */
export function isLight(palette: Palette): boolean {
  const match = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})/i.exec(palette.ground);
  if (!match) return false;
  const [red, green, blue] = match.slice(1).map((part) => parseInt(part, 16) / 255);
  return 0.2126 * red + 0.7152 * green + 0.0722 * blue > 0.5;
}

const PaletteContext = createContext<Palette>(defaultPalette);

/** Draws everything under it in a palette; a device's screens use that Mac's. */
export const PaletteProvider = PaletteContext.Provider;

export function useColors(): Palette {
  return useContext(PaletteContext);
}

const made = new WeakMap<Palette, Map<unknown, unknown>>();

/** Builds a screen's styles once per palette. */
export function useStyles<T>(make: (colors: Palette) => T): T {
  const colors = useColors();
  let built = made.get(colors);
  if (!built) {
    built = new Map();
    made.set(colors, built);
  }
  if (!built.has(make)) built.set(make, make(colors));
  return built.get(make) as T;
}

export const brand = {
  claude: '#d97757',
  codex: '#7a9dff',
  hermes: '#e0a050',
  opencode: '#a78bfa',
  pi: '#7dd3fc',
  omp: '#f97316',
  grok: '#fcfcfc',
} as const;

export type Provider = keyof typeof brand;

export const fonts = {
  ui: 'Figtree_400Regular',
  uiMedium: 'Figtree_500Medium',
  uiSemibold: 'Figtree_600SemiBold',
  uiItalic: 'Figtree_400Regular_Italic',
  mono: 'JetBrainsMono_400Regular',
} as const;

/** Text the app says is Figtree; text the machine says is mono (DESIGN.md §1). */
export function typeFor(colors: Palette) {
  return {
    title: { fontFamily: fonts.uiSemibold, fontSize: 24, letterSpacing: -0.7, color: colors.ink },
    heading: { fontFamily: fonts.uiSemibold, fontSize: 17, letterSpacing: -0.35, color: colors.ink },
    body: { fontFamily: fonts.ui, fontSize: 15, lineHeight: 22, color: colors.secondary },
    row: { fontFamily: fonts.uiMedium, fontSize: 15.5, letterSpacing: -0.15, color: colors.secondary },
    meta: { fontFamily: fonts.ui, fontSize: 13, color: colors.tertiary },
    label: { fontFamily: fonts.uiSemibold, fontSize: 13, color: colors.tertiary },
    mono: { fontFamily: fonts.mono, fontSize: 12, color: colors.tertiary },
  } satisfies Record<string, TextStyle>;
}

export function useType() {
  return useStyles(typeFor);
}

export const radius = { control: 10, row: 12, card: 14, sheet: 22 } as const;
