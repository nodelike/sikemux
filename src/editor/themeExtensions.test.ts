import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it } from "vitest";
import { themeById } from "../themes";
import type { Theme } from "../themes";
import { buildEditorThemeExtensions } from "./themeExtensions";

let view: EditorView | null = null;
afterEach(() => {
    view?.destroy();
    view = null;
});

/*
 * Mounts an editor and reads the gutter background our theme sets. CodeMirror's
 * built-in light and dark themes also colour the gutter, so only the rule
 * scoped to this theme's own class on the editor counts.
 */
function gutterBackground(theme: Theme): string {
    view = new EditorView({ state: EditorState.create({ doc: "x", extensions: buildEditorThemeExtensions(theme) }), parent: document.body });
    const rules = Array.from(document.querySelectorAll("style")).flatMap((style) => Array.from(style.sheet?.cssRules ?? []));
    const ours = rules.filter(
        (rule): rule is CSSStyleRule =>
            rule instanceof CSSStyleRule &&
            rule.selectorText.endsWith(" .cm-gutters") &&
            view!.dom.classList.contains(rule.selectorText.split(" ")[0].slice(1)) &&
            !["ͼ1", "ͼ2", "ͼ3"].includes(rule.selectorText.split(" ")[0].slice(1)),
    );
    return ours.at(-1)?.style.backgroundColor ?? "";
}

describe("the editor gutter", () => {
    it("is solid, so code scrolled sideways goes behind the line numbers", () => {
        const theme = themeById("aura");
        const background = gutterBackground(theme);

        expect(background).not.toBe("");
        expect(background).not.toBe("transparent");
    });

    it("falls back to the chrome colour when the editor itself is transparent", () => {
        const aura = themeById("aura");
        const glassy: Theme = { ...aura, editor: { ...aura.editor, bg: "transparent" } };

        expect(gutterBackground(glassy)).not.toBe("transparent");
        expect(gutterBackground(glassy)).not.toBe("");
    });
});
