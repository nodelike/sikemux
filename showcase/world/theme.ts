import curated from "../../src-tauri/crates/sikemux-core/src/server/themes/curated.json";
import type { Palette, Theme, ThemeSummary } from "../../src/themes";
import palette from "../../src/themes/defaultPalette.json";

/** The palette the core answers for its default theme, as it worked it out. */
export const DEMO_PALETTE = palette as Palette;

/** The Sikemux themes as the core lists them for a picker. */
export const DEMO_THEMES: ThemeSummary[] = (curated as Theme[]).map(
  (theme) => ({
    id: theme.id,
    name: theme.name,
    dark: theme.dark,
    source: "sikemux",
    bg: theme.chrome.bg,
    line: theme.chrome.line,
    acc: theme.chrome.acc,
    swatches: [
      theme.terminal.red,
      theme.terminal.green,
      theme.terminal.yellow,
      theme.terminal.blue,
      theme.terminal.magenta,
      theme.terminal.cyan,
    ],
  }),
);

/** What the core answers when asked for a theme by id: the default's tokens over the theme asked for. */
export function demoPalette(id: string | undefined): Palette {
  const theme = (curated as Theme[]).find((candidate) => candidate.id === id);
  return theme ? { ...DEMO_PALETTE, theme } : DEMO_PALETTE;
}
