import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { search } from "@codemirror/search";
import { EditorFindBar } from "./EditorFindBar";

afterEach(cleanup);

function mountEditor(doc: string) {
    const host = document.createElement("div");
    document.body.appendChild(host);
    return new EditorView({ state: EditorState.create({ doc, extensions: [search()] }), parent: host });
}

describe("EditorFindBar", () => {
    it("does not re-search the document when its pane re-renders", () => {
        const view = mountEditor("needle ".repeat(50));
        const dispatch = vi.spyOn(view, "dispatch");
        const bar = (getView: () => EditorView) => (
            <EditorFindBar getView={getView} documentKey="/a.ts" open replaceOpenOnMount={false} seed="needle" signal={1} onClose={() => {}} />
        );

        const { rerender } = render(bar(() => view));
        const settled = dispatch.mock.calls.length;
        expect(settled).toBeGreaterThan(0);

        rerender(bar(() => view));
        expect(dispatch.mock.calls.length).toBe(settled);

        rerender(
            <EditorFindBar getView={() => view} documentKey="/b.ts" open replaceOpenOnMount={false} seed="needle" signal={1} onClose={() => {}} />,
        );
        expect(dispatch.mock.calls.length).toBeGreaterThan(settled);
        view.destroy();
    });

    it("stops counting matches at a thousand", () => {
        const view = mountEditor("a ".repeat(5_000));
        const { container } = render(
            <EditorFindBar getView={() => view} documentKey="/a.ts" open replaceOpenOnMount={false} seed="a" signal={1} onClose={() => {}} />,
        );

        expect(container.querySelector(".ed-findbar-status")?.textContent).toBe("1 of 1000+");
        view.destroy();
    });
});
