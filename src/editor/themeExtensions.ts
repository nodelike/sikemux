import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import type { Extension } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { indentationMarkers } from "@replit/codemirror-indentation-markers";
import { tags as t } from "@lezer/highlight";
import type { Theme } from "../themes";

// The root carries it too: the line-number gutter sizes from the root, not the content.
const EDITOR_FONT_SIZE = "calc(13px * var(--editor-text-scale, 1))";

export function buildEditorThemeExtensions(theme: Theme): Extension {
    const editorTheme = EditorView.theme(
        {
            "&": { color: theme.editor.fg, backgroundColor: theme.editor.bg, fontSize: EDITOR_FONT_SIZE },
            ".cm-content": {
                caretColor: theme.editor.caret,
                fontFamily: '"JetBrainsMono Nerd Font", "JetBrains Mono", monospace',
                fontSize: EDITOR_FONT_SIZE,
            },
            ".cm-cursor, .cm-dropCursor": { borderLeftColor: theme.editor.caret },
            "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection": { backgroundColor: theme.editor.selection },
            ".cm-activeLine": { backgroundColor: theme.editor.activeLine },
            /* The gutter stays put while the code scrolls sideways under it, so
               it needs a solid ground or the code shows through the numbers. A
               theme whose editor is transparent falls back to its chrome, as
               the code-block theme does. */
            ".cm-gutters": {
                backgroundColor: theme.editor.bg === "transparent" ? theme.chrome.bg : theme.editor.bg,
                color: theme.editor.gutter,
                border: "none",
            },
            ".cm-activeLineGutter": {
                backgroundColor: "transparent",
                color: theme.editor.gutterActive,
            },
            ".cm-scroller": {
                fontFamily: '"JetBrainsMono Nerd Font", "JetBrains Mono", monospace',
                lineHeight: "1.6",
            },
            ".cm-selectionMatch": { backgroundColor: theme.chrome.accDim },
            ".cm-foldPlaceholder": {
                backgroundColor: theme.chrome.bgRaised,
                color: theme.chrome.inkDim,
                border: "none",
            },
            "&.cm-editor.cm-focused": { outline: "none" },
        },
        { dark: theme.dark },
    );

    const highlight = HighlightStyle.define([
        {
            tag: [t.keyword, t.modifier, t.controlKeyword, t.operatorKeyword],
            color: theme.highlight.keyword,
        },
        {
            tag: [t.string, t.special(t.string), t.regexp],
            color: theme.highlight.string,
        },
        {
            tag: [t.comment, t.lineComment, t.blockComment],
            color: theme.highlight.comment,
            fontStyle: "italic",
        },
        {
            tag: [t.number, t.bool, t.atom, t.null],
            color: theme.highlight.number,
        },
        {
            tag: [t.function(t.variableName), t.function(t.propertyName), t.macroName],
            color: theme.highlight.function,
        },
        {
            tag: [t.typeName, t.className, t.namespace],
            color: theme.highlight.type,
        },
        {
            tag: [t.variableName, t.definition(t.variableName)],
            color: theme.highlight.variable,
        },
        { tag: [t.propertyName], color: theme.highlight.property },
        { tag: [t.tagName], color: theme.highlight.tag },
        { tag: [t.attributeName], color: theme.highlight.number },
        {
            tag: [t.operator, t.punctuation, t.bracket, t.separator],
            color: theme.highlight.operator,
        },
        { tag: [t.heading], color: theme.highlight.heading, fontWeight: "bold" },
        { tag: [t.link, t.url], color: theme.highlight.link },
        { tag: [t.invalid], color: theme.highlight.invalid },
        {
            tag: [t.meta, t.processingInstruction],
            color: theme.highlight.meta,
        },
    ]);

    return [editorTheme, syntaxHighlighting(highlight)];
}

export function buildIndentMarkerExtensions(theme: Theme): Extension {
    return indentationMarkers({
        thickness: 1,
        // Active-block highlighting makes every caret-line / selection-line
        // move rebuild the visible indent decorations. Plain guides preserve
        // the visual affordance without making drag-selection pay that cost.
        highlightActiveBlock: false,
        colors: {
            light: theme.editor.indent,
            dark: theme.editor.indent,
            activeLight: theme.editor.indentActive,
            activeDark: theme.editor.indentActive,
        },
    });
}

export function buildEditorExtensions(theme: Theme): Extension {
    return [buildEditorThemeExtensions(theme), buildIndentMarkerExtensions(theme)];
}
