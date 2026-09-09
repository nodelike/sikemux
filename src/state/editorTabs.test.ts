import { beforeEach, describe, expect, it } from "vitest";
import { openEditorTab } from "./commands";
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
});
