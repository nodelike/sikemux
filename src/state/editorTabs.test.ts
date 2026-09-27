import { beforeEach, describe, expect, it } from "vitest";
import { keepEditorTab, openEditorTab } from "./commands";
import { getState, setState } from "./store";

const initial = getState();

beforeEach(() => setState(initial, true));

describe("openEditorTab", () => {
    it("atomically retains tabs completed by concurrent file opens", () => {
        openEditorTab("pane", "/one");
        openEditorTab("pane", "/two");
        openEditorTab("pane", "/one");

        expect(getState().editorViews.pane).toEqual({ openTabs: ["/one", "/two"], activePath: "/one" });
    });

    it("reuses the preview tab in place for the next preview", () => {
        openEditorTab("pane", "/kept");
        openEditorTab("pane", "/first", true, true);
        openEditorTab("pane", "/other");
        const replaced = openEditorTab("pane", "/second", true, true);

        expect(replaced).toBe("/first");
        expect(getState().editorViews.pane).toEqual({ openTabs: ["/kept", "/second", "/other"], activePath: "/second", preview: "/second" });
    });

    it("keeps a preview tab once it is opened for good", () => {
        openEditorTab("pane", "/file", true, true);
        openEditorTab("pane", "/file");
        openEditorTab("pane", "/next", true, true);

        expect(getState().editorViews.pane).toEqual({ openTabs: ["/file", "/next"], activePath: "/next", preview: "/next" });
    });

    it("stops replacing a tab that was kept", () => {
        openEditorTab("pane", "/file", true, true);
        keepEditorTab("pane", "/file");
        const replaced = openEditorTab("pane", "/next", true, true);

        expect(replaced).toBeNull();
        expect(getState().editorViews.pane?.openTabs).toEqual(["/file", "/next"]);
    });
});
