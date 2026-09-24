import { beforeEach, describe, expect, it } from "vitest";
import { reorderDocumentTab, reorderWindowTab } from "./commands";
import { workspaceTabDropAllowed } from "./selectors";
import { getState, setState } from "./store";
import type { TabRef, Window } from "./types";

const initial = getState();
const win = (id: string, role: Window["role"], pane: string) => ({ id, name: id, role, activePaneId: pane }) as unknown as Window;

beforeEach(() => {
    setState(initial, true);
    setState({
        windowsBySession: { s: ["term", "files", "git"] },
        windows: {
            term: win("term", "term", "p1"),
            files: win("files", "files", "p2"),
            git: win("git", "git", "p3"),
        },
        editorViews: { p2: { openTabs: ["a.ts", "b.ts", "c.ts"], activePath: "a.ts" } },
    });
});

describe("reordering workspace tabs", () => {
    it("moves a window's tab by moving the window", () => {
        reorderWindowTab("s", "git", "term", "before");
        expect(getState().windowsBySession.s).toEqual(["git", "term", "files"]);

        reorderWindowTab("s", "git", "files", "after");
        expect(getState().windowsBySession.s).toEqual(["term", "files", "git"]);
    });

    it("moves a file among the files of its editor", () => {
        reorderDocumentTab("files", "c.ts", "a.ts", "before");
        expect(getState().editorViews.p2.openTabs).toEqual(["c.ts", "a.ts", "b.ts"]);
    });

    it("changes nothing for an unknown tab or a drop onto itself", () => {
        const before = getState().windowsBySession;
        reorderWindowTab("s", "git", "git", "before");
        reorderWindowTab("s", "missing", "term", "before");
        reorderWindowTab("nope", "git", "term", "before");
        expect(getState().windowsBySession).toBe(before);
    });
});

describe("which drops the workspace strip allows", () => {
    const refs: TabRef[] = [{ id: "term" }, { id: "files", doc: "a.ts" }, { id: "files", doc: "b.ts" }, { id: "files", doc: "c.ts" }, { id: "git" }];
    const allowed = (source: TabRef, target: TabRef, placement: "before" | "after") => workspaceTabDropAllowed(refs, source, target, placement);

    it("lets a window's tab go beside any other window", () => {
        expect(allowed({ id: "git" }, { id: "term" }, "before")).toBe(true);
    });

    it("lets a window's tab go at either end of an editor's files, but not between them", () => {
        expect(allowed({ id: "git" }, { id: "files", doc: "a.ts" }, "before")).toBe(true);
        expect(allowed({ id: "term" }, { id: "files", doc: "c.ts" }, "after")).toBe(true);
        expect(allowed({ id: "git" }, { id: "files", doc: "b.ts" }, "before")).toBe(false);
        expect(allowed({ id: "git" }, { id: "files", doc: "a.ts" }, "after")).toBe(false);
    });

    it("keeps a file among its own editor's files", () => {
        expect(allowed({ id: "files", doc: "c.ts" }, { id: "files", doc: "a.ts" }, "before")).toBe(true);
        expect(allowed({ id: "files", doc: "a.ts" }, { id: "git" }, "after")).toBe(false);
        expect(allowed({ id: "files", doc: "a.ts" }, { id: "term" }, "before")).toBe(false);
    });
});
