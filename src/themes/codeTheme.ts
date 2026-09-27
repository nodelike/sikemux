import type { ThemeRegistration } from "shiki/core";
import type { Theme } from ".";

/* The palette every surface Shiki colours reads from: the diff panes and the
   code fences in a chat. Both name the theme the same way, so the same
   colours reach both. */

export function codeThemeName(theme: Theme): string {
    return `sikemux-${theme.id.replace(/[^a-z0-9_-]/gi, "-")}-${hash(JSON.stringify(theme))}`;
}

export function createCodeTheme(theme: Theme, name: string): ThemeRegistration {
    const h = theme.highlight;
    return {
        name,
        type: theme.dark ? "dark" : "light",
        colors: {
            "editor.background": theme.editor.bg === "transparent" ? theme.chrome.bg : theme.editor.bg,
            "editor.foreground": theme.editor.fg,
            "editor.selectionBackground": theme.editor.selection,
            "editor.lineHighlightBackground": theme.editor.activeLine,
            "editorLineNumber.foreground": theme.editor.gutter,
            "editorLineNumber.activeForeground": theme.editor.gutterActive,
        },
        tokenColors: [
            { scope: ["comment", "punctuation.definition.comment"], settings: { foreground: h.comment, fontStyle: "italic" } },
            { scope: ["string", "string.quoted", "string.template", "string.regexp"], settings: { foreground: h.string } },
            { scope: ["constant.numeric", "constant.language", "constant.character"], settings: { foreground: h.number } },
            { scope: ["keyword", "storage", "storage.type", "storage.modifier"], settings: { foreground: h.keyword } },
            { scope: ["entity.name.function", "support.function", "meta.function-call"], settings: { foreground: h.function } },
            { scope: ["entity.name.type", "entity.name.class", "support.type", "support.class"], settings: { foreground: h.type } },
            { scope: ["variable", "variable.other.readwrite", "variable.parameter"], settings: { foreground: h.variable } },
            {
                scope: ["variable.other.property", "support.variable.property", "meta.object-literal.key"],
                settings: { foreground: h.property },
            },
            { scope: ["entity.name.tag", "punctuation.definition.tag"], settings: { foreground: h.tag } },
            { scope: ["keyword.operator", "punctuation", "meta.brace"], settings: { foreground: h.operator } },
            { scope: ["markup.heading", "entity.name.section"], settings: { foreground: h.heading, fontStyle: "bold" } },
            {
                scope: ["markup.underline.link", "string.other.link", "constant.other.reference.link"],
                settings: { foreground: h.link, fontStyle: "underline" },
            },
            { scope: ["invalid", "invalid.illegal"], settings: { foreground: h.invalid } },
            { scope: ["meta", "meta.preprocessor", "meta.tag"], settings: { foreground: h.meta } },
        ],
    };
}

function hash(value: string): string {
    let result = 2166136261;
    for (let i = 0; i < value.length; i++) {
        result ^= value.charCodeAt(i);
        result = Math.imul(result, 16777619);
    }
    return (result >>> 0).toString(36);
}
