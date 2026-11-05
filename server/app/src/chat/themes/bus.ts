import type { Palette, Theme } from "@shared/wire.generated";
import defaultPalette from "@shared/chat/defaultPalette.json";

/* The core keeps this file the same as its own default palette. */
export const DEFAULT_PALETTE = defaultPalette as Palette;

let current: Theme = DEFAULT_PALETTE.theme;

const themeListeners = new Set<(theme: Theme) => void>();

export function currentTheme(): Theme {
  return current;
}

export function subscribeTheme(listener: (theme: Theme) => void): () => void {
  themeListeners.add(listener);
  return () => themeListeners.delete(listener);
}

function applyChrome({ theme, tokens }: Palette) {
  const c = theme.chrome;
  const root = document.documentElement.style;
  document.documentElement.classList.toggle("theme-light", !theme.dark);

  root.setProperty("--bg", c.bg);
  root.setProperty("--bg-dim", c.bgDim);
  root.setProperty("--ink", c.ink);
  root.setProperty("--ink-dim", c.inkDim);
  root.setProperty("--text-tertiary", tokens.tertiary);
  root.setProperty("--acc", c.acc);
  root.setProperty("--line", c.line);
  root.setProperty("--danger", tokens.danger);

  root.setProperty("--void", c.bgDim);
  root.setProperty("--rail", c.bg);
  root.setProperty("--rail-2", c.bgRaised);
  root.setProperty("--pane", c.bg);
  root.setProperty("--line-soft", c.bgRaised);
  root.setProperty("--ink-faint", tokens.inkFaint);
  root.setProperty("--acc-soft", c.accDim);
  root.setProperty("--live", tokens.live);
  root.setProperty("--warn", tokens.warn);
  root.setProperty("--git-modified", tokens.gitModified);
  root.setProperty("--git-added", tokens.gitAdded);
  root.setProperty("--git-deleted", tokens.gitDeleted);
  root.setProperty("--git-renamed", tokens.gitRenamed);
  root.setProperty("--cmd", tokens.cmd);
}

let settling = 0;

/* The colours on screen now, so putting the same ones on again does nothing. */
let worn = "";

/** Wears a palette the core resolved: its colours go on the page's root as variables. */
export function wearPicked(palette: Palette): void {
  const colours = JSON.stringify(palette);
  if (colours === worn) return;
  worn = colours;
  const root = document.documentElement;
  root.classList.add("theme-changing");
  current = palette.theme;
  applyChrome(palette);
  themeListeners.forEach((listener) => listener(current));
  // Transitions stay off until the new colours have been drawn once, so nothing fades between themes.
  cancelAnimationFrame(settling);
  settling = requestAnimationFrame(() => {
    settling = requestAnimationFrame(() =>
      root.classList.remove("theme-changing"),
    );
  });
}
