import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import type { Extension } from "@codemirror/state";
import { EditorView, ViewPlugin } from "@codemirror/view";
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
                paddingLeft: "6px",
            },
            ".cm-cursor, .cm-dropCursor": { borderLeftColor: theme.editor.caret },
            "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection": { backgroundColor: theme.editor.selection },
            ".cm-activeLine": { backgroundColor: theme.editor.activeLine },
            ".cm-gutters": {
                backgroundColor: "transparent",
                color: theme.editor.gutter,
                border: "none",
                borderRight: "1px solid var(--border)",
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

    return [editorTheme, syntaxHighlighting(highlight), clipCodeUnderGutter];
}

const FAR = "99999999px";

function clipFrom(left: string): string {
    return `polygon(${left} 0, ${FAR} 0, ${FAR} ${FAR}, ${left} ${FAR})`;
}

/* The gutter has no ground, so code scrolled sideways would show through the
   line numbers. Instead, cut the code and its selection off at the gutter's edge. */
const clipCodeUnderGutter = ViewPlugin.fromClass(
    class {
        private readonly onScroll = () => this.clip();

        constructor(private readonly view: EditorView) {
            view.scrollDOM.addEventListener("scroll", this.onScroll, { passive: true });
        }

        update() {
            this.clip();
        }

        private clip() {
            const { contentDOM, scrollDOM } = this.view;
            const scrolled = scrollDOM.scrollLeft;
            const layers = scrollDOM.querySelectorAll<HTMLElement>(".cm-layer");
            if (scrolled <= 0) {
                contentDOM.style.clipPath = "";
                layers.forEach((layer) => (layer.style.clipPath = ""));
                return;
            }
            const gutterWidth = scrollDOM.querySelector<HTMLElement>(".cm-gutters")?.offsetWidth ?? 0;
            contentDOM.style.clipPath = clipFrom(`${scrolled}px`);
            layers.forEach((layer) => (layer.style.clipPath = clipFrom(`${scrolled + gutterWidth}px`)));
        }

        destroy() {
            this.view.scrollDOM.removeEventListener("scroll", this.onScroll);
        }
    },
);

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
