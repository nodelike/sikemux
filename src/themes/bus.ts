import type { EditorView } from "@codemirror/view";
import type { TerminalTheme } from "../terminal/canvas";
import { coreCall } from "../api/core";
import { onCoreChanged } from "../api/coreChanged";
import { swallow } from "../state/toast";
import curated from "../../src-tauri/crates/sikemux-core/src/server/themes/curated.json";
import { DEFAULT_THEME_ID, TERMINAL_KEYS, type Palette, type Theme } from ".";

/* Until the core answers at boot, code that asks for the theme gets the default's colours. */
let current: Theme = (curated as Theme[]).find((theme) => theme.id === DEFAULT_THEME_ID) as Theme;
/* The palette the core resolved for the person's pick, worn again when a preview ends. */
let picked: Palette | null = null;
let previewing = false;
let currentOpacity = 1;

const views = new Set<EditorView>();
/** A terminal that draws with the theme's colours. */
export interface ThemedTerminal {
    setTheme(theme: TerminalTheme): void;
}

const terms = new Map<ThemedTerminal, () => boolean>();
const termThemes = new WeakMap<ThemedTerminal, TerminalTheme>();
let terminalTheme: TerminalTheme | null = null;

/** What an editor view needs from a theme change. Supplied by `editor/themeBridge`
 *  the first time a view registers, so nothing here pulls CodeMirror into the boot bundle. */
export interface EditorThemeBridge {
    rebuild(theme: Theme): void;
    push(view: EditorView): void;
}

let editorBridge: EditorThemeBridge | null = null;
let bridgeRequested = false;

export function setEditorThemeBridge(bridge: EditorThemeBridge): void {
    editorBridge = bridge;
    bridge.rebuild(current);
    views.forEach((view) => bridge.push(view));
}

function loadEditorBridge(): void {
    if (editorBridge || bridgeRequested) return;
    bridgeRequested = true;
    void import("../editor/themeBridge");
}

const themeListeners = new Set<(theme: Theme) => void>();

function applyTransparentState() {
    document.documentElement.classList.toggle("is-transparent", currentOpacity < 1);
}

const ANSI_KEYS = TERMINAL_KEYS.slice(TERMINAL_KEYS.indexOf("black"));

function terminalThemeFor(theme: Theme): TerminalTheme {
    const colors = theme.terminal;
    return {
        foreground: colors.foreground,
        background: colors.background,
        cursor: colors.cursor,
        cursorAccent: colors.cursorAccent,
        selectionBackground: colors.selectionBackground,
        ansi: ANSI_KEYS.map((key) => colors[key]),
    };
}

/* The one terminal palette, which construction and later theme changes both use. */
export function currentTerminalTheme(): TerminalTheme {
    terminalTheme ??= terminalThemeFor(current);
    return terminalTheme;
}

/** Gives a terminal the current palette if it does not have it yet. */
export function refreshTerminalTheme(term: ThemedTerminal): void {
    const theme = currentTerminalTheme();
    if (termThemes.get(term) === theme) return;
    termThemes.set(term, theme);
    term.setTheme(theme);
}

// A terminal out of sight takes the new palette once it is shown again.
function applyTerminalThemes() {
    terminalTheme = terminalThemeFor(current);
    terms.forEach((visible, term) => {
        if (visible()) refreshTerminalTheme(term);
    });
}

export function currentTheme(): Theme {
    return current;
}

export function subscribeTheme(listener: (theme: Theme) => void): () => void {
    themeListeners.add(listener);
    return () => themeListeners.delete(listener);
}

export function registerView(view: EditorView): () => void {
    loadEditorBridge();
    editorBridge?.push(view);
    views.add(view);
    return () => views.delete(view);
}

export function refreshViewTheme(view: EditorView): void {
    editorBridge?.push(view);
}

export function registerTerminal(term: ThemedTerminal, visible: () => boolean): () => void {
    refreshTerminalTheme(term);
    terms.set(term, visible);
    return () => terms.delete(term);
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
    for (const [key, color] of Object.entries(theme.editor)) root.setProperty(`--editor-${key}`, color);
    for (const [key, color] of Object.entries(theme.highlight)) root.setProperty(`--syntax-${key}`, color);
    root.setProperty("--cmd", tokens.cmd);
    root.setProperty("--terminal-background", theme.terminal.background);
    TERMINAL_KEYS.slice(TERMINAL_KEYS.indexOf("black")).forEach((key, index) => root.setProperty(`--terminal-ansi-${index}`, theme.terminal[key]));

    applyTransparentState();
}

let settling = 0;

function applyThemeObject(palette: Palette): void {
    const next = palette.theme;
    const root = document.documentElement;
    root.classList.add("theme-changing");
    current = next;
    applyChrome(palette);
    editorBridge?.rebuild(next);
    if (editorBridge) views.forEach(editorBridge.push);
    applyTerminalThemes();
    themeListeners.forEach((listener) => listener(next));
    // Transitions stay off until the new colours have been drawn once, so nothing fades between themes.
    cancelAnimationFrame(settling);
    settling = requestAnimationFrame(() => {
        settling = requestAnimationFrame(() => root.classList.remove("theme-changing"));
    });
}

/* The colours on screen now, so putting the same ones on again does nothing. */
let worn = "";

function wear(palette: Palette): void {
    const colours = JSON.stringify(palette);
    if (colours === worn) return;
    worn = colours;
    applyThemeObject(palette);
}

/** Wears the palette the core resolved for the person's pick. A preview stays on
 *  until the pick itself changes, as it does once a previewed theme is kept. */
export function wearPicked(palette: Palette): void {
    if (picked?.theme.id !== palette.theme.id) previewing = false;
    picked = palette;
    if (!previewing) wear(palette);
}

/** Keeps the window in the person's theme as the core resolves it, for as long as the page runs. */
export function watchTheme(): () => void {
    let live = true;
    const read = () =>
        coreCall<Palette>("theme")
            .then((palette) => {
                if (live) wearPicked(palette);
            })
            .catch(swallow("read your theme"));
    const stop = onCoreChanged("preferences", () => void read());
    void read();
    return () => {
        live = false;
        stop();
    };
}

/** Wears a theme that is not saved yet, as a theme editor changes it, until {@link endPreview}. */
export function previewTheme(theme: Theme): void {
    previewing = true;
    coreCall<Palette>("themePalette", { theme })
        .then((palette) => {
            if (previewing) wear(palette);
        })
        .catch(swallow("preview the theme"));
}

/** Wears one of the themes to pick from, by id, until {@link endPreview}. */
export function previewThemeId(id: string): void {
    previewing = true;
    coreCall<Palette>("theme", { id })
        .then((palette) => {
            if (previewing) wear(palette);
        })
        .catch(swallow("preview the theme"));
}

export function endPreview(): void {
    previewing = false;
    if (picked) wear(picked);
}

export function applyWindowOpacity(opacity: number): void {
    const v = Math.max(0, Math.min(1, opacity));
    if (v === currentOpacity) return;
    currentOpacity = v;
    document.documentElement.style.setProperty("--window-opacity", String(v));
    applyTransparentState();
    applyTerminalThemes();
}
