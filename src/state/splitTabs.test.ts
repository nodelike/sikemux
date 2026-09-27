import { describe, expect, it } from "vitest";
import { separatePane, splitWithTab } from "./commands";
import { collectPanes } from "./layout";
import { paneToSeparate, tabSplitAllowed } from "./selectors";
import { getState, setState } from "./store";
import type { LayoutNode, Window } from "./types";

const initial = getState();
const pane = (id: string, kind: "terminal" | "agent" | "git" | "editor" = "terminal"): LayoutNode => ({
    type: "pane",
    id,
    cwd: "/p",
    kind,
    title: id,
});
const win = (id: string, role: Window["role"], root: LayoutNode, extra: Partial<Window> = {}): Window => ({
    id,
    name: id,
    role,
    root,
    activePaneId: collectPanes(root)[0].id,
    ...extra,
});

function place(showing: string, ...windows: Window[]): void {
    setState(initial, true);
    const session = getState().sessions[getState().activeSessionId];
    setState({
        windows: Object.fromEntries(windows.map((w) => [w.id, w])),
        windowsBySession: { [session.id]: windows.map((w) => w.id) },
        sessions: { [session.id]: { ...session, kind: "project", cwd: "/p", activeWindowId: showing } },
        agents: {},
    });
}

const sessionId = () => getState().activeSessionId;
const shown = () => getState().windows[getState().sessions[sessionId()].activeWindowId];
const tabs = () => getState().windowsBySession[sessionId()];

describe("splitting two tabs into one", () => {
    it("puts the dragged terminal on the side it was dropped, both still running", () => {
        place("one", win("one", "term", pane("p1")), win("two", "term", pane("p2")));

        splitWithTab(sessionId(), { id: "two" }, "left");

        expect(tabs()).toEqual(["one"]);
        expect(shown().root).toMatchObject({ type: "split", dir: "row", sizes: [0.5, 0.5], children: [{ id: "p2" }, { id: "p1" }] });
        expect(shown().activePaneId).toBe("p2");
    });

    it("keeps an agent's tab, so the agent is still found by it", () => {
        place("term", win("term", "term", pane("p1")), win("agent", "agent", pane("a1", "agent")));

        splitWithTab(sessionId(), { id: "agent" }, "right");

        expect(tabs()).toEqual(["agent"]);
        expect(shown()).toMatchObject({ id: "agent", role: "agent" });
        expect(collectPanes(shown().root).map((p) => p.id)).toEqual(["p1", "a1"]);
    });

    it("refuses two agents, tool tabs, pinned tabs and a tab onto itself", () => {
        place("a", win("a", "agent", pane("a1", "agent")), win("b", "agent", pane("b1", "agent")), win("git", "git", pane("g", "git")));
        expect(tabSplitAllowed(getState(), sessionId(), { id: "b" })).toBe(false);
        expect(tabSplitAllowed(getState(), sessionId(), { id: "git" })).toBe(false);
        expect(tabSplitAllowed(getState(), sessionId(), { id: "a" })).toBe(false);

        place("one", win("one", "term", pane("p1")), win("pinned", "term", pane("p2"), { fixed: true }));
        expect(tabSplitAllowed(getState(), sessionId(), { id: "pinned" })).toBe(false);

        splitWithTab(sessionId(), { id: "pinned" }, "left");
        expect(tabs()).toEqual(["one", "pinned"]);
    });

    it("leaves a tab alone once it is two stacked, and keeps a split tab from being dragged in", () => {
        const stacked: LayoutNode = { type: "split", id: "s", dir: "column", children: [pane("p1"), pane("p2")], sizes: [0.5, 0.5] };
        place("one", win("one", "term", stacked), win("two", "named", pane("p3")));
        expect(tabSplitAllowed(getState(), sessionId(), { id: "two" })).toBe(false);

        place("one", win("one", "term", pane("p1")), win("two", "term", stacked));
        expect(tabSplitAllowed(getState(), sessionId(), { id: "two" })).toBe(false);
    });
});

describe("splitting in every direction", () => {
    it("stacks a tab above or below the one on screen", () => {
        place("one", win("one", "term", pane("p1")), win("two", "term", pane("p2")));

        splitWithTab(sessionId(), { id: "two" }, "bottom");

        expect(shown().root).toMatchObject({ type: "split", dir: "column", children: [{ id: "p1" }, { id: "p2" }] });
    });

    it("fits three across, in the order they were dropped, sharing the space evenly", () => {
        place(
            "one",
            win("one", "term", pane("p1")),
            win("two", "term", pane("p2")),
            win("three", "term", pane("p3")),
            win("four", "term", pane("p4")),
        );

        splitWithTab(sessionId(), { id: "two" }, "right", "p1");
        splitWithTab(sessionId(), { id: "three" }, "left", "p2");

        expect(tabs()).toEqual(["one", "four"]);
        expect(shown().root).toMatchObject({ dir: "row", children: [{ id: "p1" }, { id: "p3" }, { id: "p2" }] });
        const root = shown().root;
        const sizes = root.type === "split" ? root.sizes : [];
        expect(sizes.map((size) => size.toFixed(3))).toEqual(["0.333", "0.333", "0.333"]);
        expect(tabSplitAllowed(getState(), sessionId(), { id: "four" })).toBe(false);
    });

    it("stacks only two, and a row takes no pane above or below", () => {
        place("one", win("one", "term", pane("p1")), win("two", "term", pane("p2")), win("three", "term", pane("p3")));
        splitWithTab(sessionId(), { id: "two" }, "right", "p1");

        splitWithTab(sessionId(), { id: "three" }, "bottom", "p1");

        expect(tabs()).toEqual(["one", "three"]);
        expect(shown().root).toMatchObject({ dir: "row", children: [{ id: "p1" }, { id: "p2" }] });
    });

    it("splits beside the whole tab when the pane named is not in it", () => {
        place("one", win("one", "term", pane("p1")), win("two", "term", pane("p2")));

        splitWithTab(sessionId(), { id: "two" }, "left", "somewhere-else");

        expect(shown().root).toMatchObject({ dir: "row", children: [{ id: "p2" }, { id: "p1" }] });
    });
});

describe("moving a terminal back to its own tab", () => {
    it("takes the focused terminal out, just after the tab it was in", () => {
        place("one", win("one", "term", pane("p1")), win("two", "term", pane("p2")), win("three", "term", pane("p4")));
        splitWithTab(sessionId(), { id: "two" }, "right");

        separatePane("one");

        const ids = tabs();
        expect(ids).toHaveLength(3);
        expect(ids[0]).toBe("one");
        expect(getState().windows.one.root).toMatchObject({ id: "p1" });
        const separated = getState().windows[ids[1]];
        expect(separated).toMatchObject({ role: "term", root: { id: "p2" }, activePaneId: "p2" });
        expect(shown().id).toBe(separated.id);
    });

    it("never offers an agent or a tab with nothing split", () => {
        const agentTab = win("agent", "agent", {
            type: "split",
            id: "s",
            dir: "row",
            children: [pane("p1"), pane("a1", "agent")],
            sizes: [0.5, 0.5],
        });
        expect(paneToSeparate({ ...agentTab, activePaneId: "a1" }, {})).toBeNull();
        expect(paneToSeparate({ ...agentTab, activePaneId: "p1" }, {})?.id).toBe("p1");
        expect(paneToSeparate(win("one", "term", pane("p1")), {})).toBeNull();
    });
});

describe("splitting a file beside the tab on screen", () => {
    function editorWith(...paths: string[]): Window {
        const editor = win("files", "files", pane("ed", "editor"));
        setState({ editorViews: { ed: { openTabs: paths, activePath: paths[0] } } });
        return editor;
    }

    it("moves the file out of the editor into a pane of its own beside the agent", () => {
        place("agent", win("agent", "agent", pane("a1", "agent")), win("files", "files", pane("ed", "editor")));
        editorWith("/p/a.ts", "/p/b.ts");

        splitWithTab(sessionId(), { id: "files", doc: "/p/a.ts" }, "right");

        expect(tabs()).toEqual(["agent", "files"]);
        const [agentPane, filePane] = collectPanes(shown().root);
        expect(agentPane.id).toBe("a1");
        expect(filePane).toMatchObject({ kind: "editor" });
        expect(getState().editorViews[filePane.id]).toEqual({ openTabs: ["/p/a.ts"], activePath: "/p/a.ts" });
        expect(shown().activePaneId).toBe(filePane.id);
    });

    it("keeps a file with unsaved changes in its editor", () => {
        place("term", win("term", "term", pane("p1")), win("files", "files", pane("ed", "editor")));
        editorWith("/p/a.ts");
        setState({ dirtyEditorPaths: { ed: ["/p/a.ts"] } });

        expect(tabSplitAllowed(getState(), sessionId(), { id: "files", doc: "/p/a.ts" })).toBe(false);
    });

    it("sends the file back to the editor when it is moved out", () => {
        place("term", win("term", "term", pane("p1")), win("files", "files", pane("ed", "editor")));
        editorWith("/p/a.ts", "/p/b.ts");
        splitWithTab(sessionId(), { id: "files", doc: "/p/a.ts" }, "left");
        const filePane = collectPanes(shown().root)[0];

        separatePane("term");

        expect(getState().windows.term.root).toMatchObject({ id: "p1" });
        expect(getState().editorViews[filePane.id]).toBeUndefined();
        expect(shown().id).toBe("files");
    });
});
