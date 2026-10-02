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
    it("shades the line numbers with the translucent gutter tone, never a solid ground", () => {
        expect(gutterBackground(themeById("aura"))).toBe("var(--surface-gutter)");
    });

    it("cuts code scrolled sideways off at the gutter's edge", () => {
        gutterBackground(themeById("aura"));
        const scroller = view!.scrollDOM;
        Object.defineProperty(scroller, "scrollLeft", { configurable: true, value: 40 });
        scroller.dispatchEvent(new Event("scroll"));

        expect(view!.contentDOM.style.clipPath).toContain("polygon(40px 0");

        Object.defineProperty(scroller, "scrollLeft", { configurable: true, value: 0 });
        scroller.dispatchEvent(new Event("scroll"));

        expect(view!.contentDOM.style.clipPath).toBe("");
    });
});
