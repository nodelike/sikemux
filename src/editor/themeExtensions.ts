import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import type { Extension } from "@codemirror/state";
import { EditorView, ViewPlugin } from "@codemirror/view";
import { indentationMarkers } from "@replit/codemirror-indentation-markers";
import { tags as t } from "@lezer/highlight";
import type { ThemeEditor, ThemeHighlight } from "../themes";

// The root carries it too: the line-number gutter sizes from the root, not the content.
const EDITOR_FONT_SIZE = "calc(13px * var(--editor-text-scale, 1))";

const editorColour = (key: keyof ThemeEditor) => `var(--editor-${key})`;
const syntaxColour = (key: keyof ThemeHighlight) => `var(--syntax-${key})`;

/** Colours come from variables the theme bus sets, so only a change between light and dark rebuilds this. */
export function buildEditorThemeExtensions(dark: boolean): Extension {
    const editorTheme = EditorView.theme(
        {
            "&": { color: editorColour("fg"), backgroundColor: editorColour("bg"), fontSize: EDITOR_FONT_SIZE },
            ".cm-content": {
                caretColor: editorColour("caret"),
                fontFamily: '"JetBrainsMono Nerd Font", "JetBrains Mono", monospace',
                fontSize: EDITOR_FONT_SIZE,
                paddingLeft: "6px",
            },
            ".cm-cursor, .cm-dropCursor": { borderLeftColor: editorColour("caret") },
            "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection": { backgroundColor: editorColour("selection") },
            ".cm-activeLine": { backgroundColor: editorColour("activeLine") },
            ".cm-gutters": {
                backgroundColor: "var(--surface-gutter)",
                color: editorColour("gutter"),
                border: "none",
                borderRight: "1px solid var(--border)",
            },
            ".cm-activeLineGutter": {
                backgroundColor: "transparent",
                color: editorColour("gutterActive"),
            },
            ".cm-scroller": {
                fontFamily: '"JetBrainsMono Nerd Font", "JetBrains Mono", monospace',
                lineHeight: "1.6",
            },
            ".cm-selectionMatch": { backgroundColor: "var(--acc-dim)" },
            ".cm-foldPlaceholder": {
                backgroundColor: "var(--bg-raised)",
                color: "var(--ink-dim)",
                border: "none",
            },
            "&.cm-editor.cm-focused": { outline: "none" },
        },
        { dark },
    );

    const highlight = HighlightStyle.define([
        {
            tag: [t.keyword, t.modifier, t.controlKeyword, t.operatorKeyword],
            color: syntaxColour("keyword"),
        },
        {
            tag: [t.string, t.special(t.string), t.regexp],
            color: syntaxColour("string"),
        },
        {
            tag: [t.comment, t.lineComment, t.blockComment],
            color: syntaxColour("comment"),
            fontStyle: "italic",
        },
        {
            tag: [t.number, t.bool, t.atom, t.null],
            color: syntaxColour("number"),
        },
        {
            tag: [t.function(t.variableName), t.function(t.propertyName), t.macroName],
            color: syntaxColour("function"),
        },
        {
            tag: [t.typeName, t.className, t.namespace],
            color: syntaxColour("type"),
        },
        {
            tag: [t.variableName, t.definition(t.variableName)],
            color: syntaxColour("variable"),
        },
        { tag: [t.propertyName], color: syntaxColour("property") },
        { tag: [t.tagName], color: syntaxColour("tag") },
        { tag: [t.attributeName], color: syntaxColour("number") },
        {
            tag: [t.operator, t.punctuation, t.bracket, t.separator],
            color: syntaxColour("operator"),
        },
        { tag: [t.heading], color: syntaxColour("heading"), fontWeight: "bold" },
        { tag: [t.link, t.url], color: syntaxColour("link") },
        { tag: [t.invalid], color: syntaxColour("invalid") },
        {
            tag: [t.meta, t.processingInstruction],
            color: syntaxColour("meta"),
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

export const indentMarkerExtensions: Extension = indentationMarkers({
    thickness: 1,
    // Active-block highlighting makes every caret-line / selection-line
    // move rebuild the visible indent decorations. Plain guides preserve
    // the visual affordance without making drag-selection pay that cost.
    highlightActiveBlock: false,
    colors: {
        light: editorColour("indent"),
        dark: editorColour("indent"),
        activeLight: editorColour("indentActive"),
        activeDark: editorColour("indentActive"),
    },
});
