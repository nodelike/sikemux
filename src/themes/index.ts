import type { Theme } from "../api/generated/wire";

export type { Palette, Theme, ThemeChrome, ThemeEditor, ThemeHighlight, ThemeSummary, ThemeTerminal, UiTokens } from "../api/generated/wire";

/** The theme a person has until they pick another, as the core falls back to it. */
export const DEFAULT_THEME_ID = "aura-noir";

export const NO_THEMES: readonly Theme[] = [];

export const CHROME_KEYS = ["bg", "bgDim", "bgRaised", "ink", "inkDim", "inkMuted", "acc", "accLine", "accDim", "line", "hl", "danger"] as const;
export const EDITOR_KEYS = ["fg", "bg", "caret", "selection", "activeLine", "gutter", "gutterActive", "indent", "indentActive"] as const;
export const HIGHLIGHT_KEYS = [
    "keyword",
    "string",
    "comment",
    "number",
    "function",
    "type",
    "variable",
    "property",
    "tag",
    "operator",
    "link",
    "invalid",
    "meta",
    "heading",
] as const;
export const TERMINAL_KEYS = [
    "background",
    "foreground",
    "cursor",
    "cursorAccent",
    "selectionBackground",
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "brightBlack",
    "brightRed",
    "brightGreen",
    "brightYellow",
    "brightBlue",
    "brightMagenta",
    "brightCyan",
    "brightWhite",
] as const;

/** Deep-clone a theme so its colour maps can be mutated independently of the source. */
export function cloneTheme(src: Theme, overrides?: Partial<Pick<Theme, "id" | "name" | "dark">>): Theme {
    return {
        ...src,
        ...overrides,
        chrome: { ...src.chrome },
        editor: { ...src.editor },
        highlight: { ...src.highlight },
        terminal: { ...src.terminal },
    };
}

export function newCustomThemeId(): string {
    return `custom-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`;
}

export type ThemeGroupKey = "chrome" | "editor" | "highlight" | "terminal";

export interface ThemeField {
    key: string;
    label: string;
}

export interface ThemeGroup {
    key: ThemeGroupKey;
    label: string;
    hint: string;
    fields: ThemeField[];
}

const LABELS: Record<string, string> = {
    bg: "background",
    bgDim: "background dim",
    bgRaised: "background raised",
    ink: "text",
    inkDim: "text dim",
    inkMuted: "text muted",
    acc: "accent",
    accLine: "accent border",
    accDim: "accent wash",
    line: "border",
    hl: "row highlight",
    danger: "danger",
    fg: "foreground",
    caret: "caret",
    selection: "selection",
    activeLine: "active line",
    gutter: "gutter",
    gutterActive: "gutter active",
    indent: "indent guide",
    indentActive: "indent active",
    keyword: "keyword",
    string: "string",
    comment: "comment",
    number: "number",
    function: "function",
    type: "type",
    variable: "variable",
    property: "property",
    tag: "tag",
    operator: "operator",
    link: "link",
    invalid: "invalid",
    meta: "meta",
    heading: "heading",
    background: "background",
    foreground: "foreground",
    cursor: "cursor",
    cursorAccent: "cursor text",
    selectionBackground: "selection",
    black: "black",
    red: "red",
    green: "green",
    yellow: "yellow",
    blue: "blue",
    magenta: "magenta",
    cyan: "cyan",
    white: "white",
    brightBlack: "bright black",
    brightRed: "bright red",
    brightGreen: "bright green",
    brightYellow: "bright yellow",
    brightBlue: "bright blue",
    brightMagenta: "bright magenta",
    brightCyan: "bright cyan",
    brightWhite: "bright white",
};

const toFields = (keys: readonly string[]): ThemeField[] => keys.map((key) => ({ key, label: LABELS[key] ?? key }));

export const THEME_GROUPS: ThemeGroup[] = [
    { key: "chrome", label: "interface", hint: "Window chrome, rails, accents.", fields: toFields(CHROME_KEYS) },
    { key: "editor", label: "editor", hint: "Code surface, caret, gutters.", fields: toFields(EDITOR_KEYS) },
    { key: "highlight", label: "syntax", hint: "Token colours in the code editor.", fields: toFields(HIGHLIGHT_KEYS) },
    { key: "terminal", label: "terminal", hint: "terminal palette — 16 ANSI colours plus cursor & selection.", fields: toFields(TERMINAL_KEYS) },
];
