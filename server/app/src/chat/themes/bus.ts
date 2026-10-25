import curated from "../../../../../src-tauri/crates/sikemux-core/src/server/themes/curated.json";
import {
  DEFAULT_THEME_ID,
  TERMINAL_KEYS,
  type Palette,
  type Theme,
} from "./index";

/* Until the page wears its palette, code that asks for the theme gets the default's colours. */
let current: Theme = (curated as Theme[]).find(
  (theme) => theme.id === DEFAULT_THEME_ID,
) as Theme;

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
  root.setProperty("--bg-raised", c.bgRaised);
  root.setProperty("--ink", c.ink);
  root.setProperty("--ink-dim", c.inkDim);
  root.setProperty("--ink-muted", tokens.tertiary);
  root.setProperty("--text-tertiary", tokens.tertiary);
  root.setProperty("--acc", c.acc);
  root.setProperty("--acc-line", c.accLine);
  root.setProperty("--acc-dim", c.accDim);
  root.setProperty("--line", c.line);
  root.setProperty("--hl", c.hl);
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
  root.setProperty("--git-untracked", tokens.gitUntracked);
  root.setProperty("--git-added", tokens.gitAdded);
  root.setProperty("--git-deleted", tokens.gitDeleted);
  root.setProperty("--git-renamed", tokens.gitRenamed);
  for (const [key, color] of Object.entries(theme.editor))
    root.setProperty(`--editor-${key}`, color);
  for (const [key, color] of Object.entries(theme.highlight))
    root.setProperty(`--syntax-${key}`, color);
  root.setProperty("--cmd", tokens.cmd);
  root.setProperty("--terminal-background", theme.terminal.background);
  TERMINAL_KEYS.slice(TERMINAL_KEYS.indexOf("black")).forEach((key, index) =>
    root.setProperty(`--terminal-ansi-${index}`, theme.terminal[key]),
  );
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
