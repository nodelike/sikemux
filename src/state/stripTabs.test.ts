import { describe, expect, it } from "vitest";
import { activeTabRef, expandTabRefs, nextInCycle, roleHasTab, selectSwipeOrder, stripOrder, tabRefKey } from "./selectors";
import type { StoreState } from "./store";

const win = (id: string, role: string) => ({ id, role, activePaneId: `${id}-pane` }) as unknown as StoreState["windows"][string];

describe("roleHasTab", () => {
    /*
     * The rail reaches these and the stage renders them, so a tab would be a
     * second handle on one surface — "Git" in the rail and "Git" in the strip
     * meant the same screen.
     */
    it("denies a window tab to the roles the workspace rail drives", () => {
        expect(roleHasTab("diff")).toBe(false);
        expect(roleHasTab("search")).toBe(false);
        expect(roleHasTab("git")).toBe(false);
    });

    /*
     * An editor is not one surface. The rail browses the tree, but each open
     * document is its own thing to switch between, so an editor contributes a
     * tab per document rather than none — see the expandTabRefs cases below.
     */
    it("leaves the editor out of the window rule, since it expands per document", () => {
        expect(roleHasTab("files")).toBe(true);
    });

    it("keeps a tab for every role nothing else can reach", () => {
        for (const role of ["term", "aws", "rundeck", "bruno", "ssh-config", "named"]) {
            expect(roleHasTab(role)).toBe(true);
        }
    });
});

describe("selectSwipeOrder", () => {
    it("skips the windows the strip has no tab for", () => {
        const state = {
            windowsBySession: { s: ["a1", "e1", "g1", "a2"] },
            windows: { a1: win("a1", "agent"), e1: win("e1", "files"), g1: win("g1", "git"), a2: win("a2", "agent") },
            editorViews: { "e1-pane": { openTabs: [], activePath: null } },
            brunoViews: {},
        } as unknown as StoreState;

        expect(selectSwipeOrder(state, "s")).toEqual(["a1", "a2"]);
    });
});

describe("expandTabRefs", () => {
    it("leaves out rail-driven windows and keeps the rest", () => {
        const refs = expandTabRefs(["t1", "e1", "d1", "s1", "g1"], {
            t1: win("t1", "term"),
            e1: win("e1", "files"),
            d1: win("d1", "diff"),
            s1: win("s1", "search"),
            g1: win("g1", "git"),
        });

        expect(refs.map(tabRefKey)).toEqual(["t1"]);
    });

    it("expands an editor into one tab per open document, in their open order", () => {
        const refs = expandTabRefs(["e1"], { e1: win("e1", "files") }, { "e1-pane": { openTabs: ["/a.ts", "/b.ts"], activePath: "/b.ts" } });

        expect(refs.map(tabRefKey)).toEqual(["e1:/a.ts", "e1:/b.ts"]);
    });

    it("gives an editor holding nothing no tab at all", () => {
        const refs = expandTabRefs(["e1"], { e1: win("e1", "files") }, { "e1-pane": { openTabs: [], activePath: null } });

        expect(refs).toEqual([]);
    });

    it("keeps a document's tab beside the terminals and agents it shares a strip with", () => {
        const refs = expandTabRefs(
            ["t1", "e1", "a1"],
            { t1: win("t1", "term"), e1: win("e1", "files"), a1: win("a1", "agent") },
            { "e1-pane": { openTabs: ["/a.ts"], activePath: "/a.ts" } },
        );

        expect(refs.map(tabRefKey)).toEqual(["t1", "e1:/a.ts", "a1"]);
    });

    it("drops ids with no record", () => {
        const refs = expandTabRefs(["t1", "gone"], { t1: win("t1", "term") });

        expect(refs.map(tabRefKey)).toEqual(["t1"]);
    });

    it("yields no tabs for a project holding only rail-driven surfaces", () => {
        const refs = expandTabRefs(["e1", "d1"], { e1: win("e1", "files"), d1: win("d1", "diff") });

        expect(refs).toEqual([]);
    });
});

describe("activeTabRef", () => {
    const session = (over: Record<string, unknown> = {}) => ({ activeWindowId: "e1", ...over }) as unknown as Parameters<typeof activeTabRef>[0];

    it("resolves an active editor to the document it is showing", () => {
        const ref = activeTabRef(session(), { e1: win("e1", "files") }, { "e1-pane": { openTabs: ["/a.ts", "/b.ts"], activePath: "/b.ts" } });

        expect(ref && tabRefKey(ref)).toBe("e1:/b.ts");
    });

    /*
     * An editor showing nothing has no document tab to point at, but its layer
     * still has to render the empty state, so it stays a window ref.
     */
    it("keeps an empty editor a window ref so its layer still renders", () => {
        const ref = activeTabRef(session(), { e1: win("e1", "files") }, { "e1-pane": { openTabs: [], activePath: null } });

        expect(ref && tabRefKey(ref)).toBe("e1");
    });

    it("still names the window itself for every other role", () => {
        const ref = activeTabRef(session({ activeWindowId: "t1" }), { t1: win("t1", "term") }, {});

        expect(ref && tabRefKey(ref)).toBe("t1");
    });
});

const storeState = (over: Partial<StoreState>): StoreState =>
    ({
        sessions: {},
        windows: {},
        agents: {},
        editorViews: {},
        brunoViews: {},
        windowsBySession: {},
        ...over,
    }) as unknown as StoreState;

describe("nextInCycle", () => {
    it("has nowhere to go in an empty list", () => {
        expect(nextInCycle({ ids: [], activeId: null }, 1)).toBeNull();
    });

    it("wraps at both ends", () => {
        const order = { ids: ["a", "b", "c"], activeId: "c" };
        expect(nextInCycle(order, 1)).toBe("a");
        expect(nextInCycle({ ...order, activeId: "a" }, -1)).toBe("c");
    });

    /*
     * A strip whose active id is gone still has to move somewhere, so the walk
     * starts from the first entry rather than refusing.
     */
    it("starts from the first entry when the active id is not in the list", () => {
        expect(nextInCycle({ ids: ["a", "b"], activeId: "gone" }, 1)).toBe("b");
        expect(nextInCycle({ ids: ["a", "b"], activeId: null }, 1)).toBe("b");
    });

    it("returns the only entry rather than nothing", () => {
        expect(nextInCycle({ ids: ["a"], activeId: "a" }, 1)).toBe("a");
    });
});

describe("stripOrder", () => {
    it("reads the workspace strip as keys with the active one named", () => {
        const state = storeState({
            sessions: { s1: { id: "s1", kind: "project", activeWindowId: "t2" } },
            windows: { t1: win("t1", "term"), t2: win("t2", "term"), a1: win("a1", "agent") },
            windowsBySession: { s1: ["t1", "t2", "a1"] },
        } as unknown as Partial<StoreState>);

        expect(stripOrder(state, { kind: "workspace", sessionId: "s1" })).toEqual({
            ids: ["t1", "t2", "a1"],
            activeId: "t2",
        });
    });

    /* ⌥. inside an agent walks the agent windows only, by window id, since that is what gets selected. */
    it("reads a session's agent windows", () => {
        const state = storeState({
            sessions: { s1: { id: "s1", activeWindowId: "w2" } },
            windows: { w1: win("w1", "agent"), t1: win("t1", "term"), w2: win("w2", "agent") },
            windowsBySession: { s1: ["w1", "t1", "w2"] },
        } as unknown as Partial<StoreState>);

        expect(stripOrder(state, { kind: "agents", sessionId: "s1" })).toEqual({ ids: ["w1", "w2"], activeId: "w2" });
    });

    /*
     * ⌥. inside a terminal walks terminals only, so a git or editor window
     * sharing the session must not land in the list.
     */
    it("reads only the terminal windows, and drops an active window that is not one", () => {
        const state = storeState({
            sessions: { s1: { id: "s1", activeWindowId: "g1" } },
            windows: { t1: win("t1", "term"), g1: win("g1", "git"), t2: win("t2", "term") },
            windowsBySession: { s1: ["t1", "g1", "t2"] },
        } as unknown as Partial<StoreState>);

        expect(stripOrder(state, { kind: "terminals", sessionId: "s1" })).toEqual({ ids: ["t1", "t2"], activeId: null });
    });

    it("reads an editor pane's documents", () => {
        const state = storeState({
            editorViews: { p1: { openTabs: ["/a.ts", "/b.ts"], activePath: "/b.ts" } },
        } as unknown as Partial<StoreState>);

        expect(stripOrder(state, { kind: "documents", paneId: "p1" })).toEqual({ ids: ["/a.ts", "/b.ts"], activeId: "/b.ts" });
    });

    it("reads an absent list as empty rather than throwing", () => {
        const state = storeState({});
        expect(stripOrder(state, { kind: "documents", paneId: "missing" })).toEqual({ ids: [], activeId: null });
        expect(stripOrder(state, { kind: "agents", sessionId: "missing" })).toEqual({ ids: [], activeId: null });
    });
});
