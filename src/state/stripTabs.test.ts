import { describe, expect, it } from "vitest";
import { activeTabRef, expandTabRefs, roleHasTab, tabRefKey } from "./selectors";
import type { StoreState } from "./store";

const win = (id: string, role: string) => ({ id, role, activePaneId: `${id}-pane` }) as unknown as StoreState["windows"][string];
const agent = () => ({}) as StoreState["agents"][string];

describe("roleHasTab", () => {
    /*
     * The rail reaches these and the stage renders them, so a tab would be a
     * second handle on one surface — "Changes" in the rail and "Diff" in the
     * strip meant the same diff.
     */
    it("denies a window tab to the roles the workspace rail drives", () => {
        expect(roleHasTab("diff")).toBe(false);
        expect(roleHasTab("search")).toBe(false);
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
        for (const role of ["term", "git", "aws", "rundeck", "bruno", "ssh-config", "named"]) {
            expect(roleHasTab(role)).toBe(true);
        }
    });
});

describe("expandTabRefs", () => {
    it("leaves out rail-driven windows and keeps the rest", () => {
        const refs = expandTabRefs(
            ["t1", "e1", "d1", "s1", "g1"],
            [],
            { t1: win("t1", "term"), e1: win("e1", "files"), d1: win("d1", "diff"), s1: win("s1", "search"), g1: win("g1", "git") },
            {},
        );

        expect(refs.map(tabRefKey)).toEqual(["window:t1", "window:g1"]);
    });

    it("expands an editor into one tab per open document, in their open order", () => {
        const refs = expandTabRefs(["e1"], [], { e1: win("e1", "files") }, {}, { "e1-pane": { openTabs: ["/a.ts", "/b.ts"], activePath: "/b.ts" } });

        expect(refs.map(tabRefKey)).toEqual(["file:e1:/a.ts", "file:e1:/b.ts"]);
    });

    it("gives an editor holding nothing no tab at all", () => {
        const refs = expandTabRefs(["e1"], [], { e1: win("e1", "files") }, {}, { "e1-pane": { openTabs: [], activePath: null } });

        expect(refs).toEqual([]);
    });

    it("keeps a document's tab beside the terminals and agents it shares a strip with", () => {
        const refs = expandTabRefs(
            ["t1", "e1"],
            ["a1"],
            { t1: win("t1", "term"), e1: win("e1", "files") },
            { a1: agent() },
            { "e1-pane": { openTabs: ["/a.ts"], activePath: "/a.ts" } },
        );

        expect(refs.map(tabRefKey)).toEqual(["window:t1", "file:e1:/a.ts", "agent:a1"]);
    });

    it("orders windows before agents and drops ids with no record", () => {
        const refs = expandTabRefs(["t1", "gone"], ["a1", "vanished"], { t1: win("t1", "term") }, { a1: agent() });

        expect(refs.map(tabRefKey)).toEqual(["window:t1", "agent:a1"]);
    });

    it("yields no tabs for a project holding only rail-driven surfaces", () => {
        const refs = expandTabRefs(["e1", "d1"], [], { e1: win("e1", "files"), d1: win("d1", "diff") }, {});

        expect(refs).toEqual([]);
    });
});

describe("activeTabRef", () => {
    const session = (over: Record<string, unknown> = {}) =>
        ({ kind: "project", view: "windows", activeWindowId: "e1", activeAgentId: null, ...over }) as unknown as Parameters<typeof activeTabRef>[0];

    it("resolves an active editor to the document it is showing", () => {
        const ref = activeTabRef(session(), { e1: win("e1", "files") }, { "e1-pane": { openTabs: ["/a.ts", "/b.ts"], activePath: "/b.ts" } });

        expect(ref && tabRefKey(ref)).toBe("file:e1:/b.ts");
    });

    it("marks nothing active when the editor holds no document", () => {
        expect(activeTabRef(session(), { e1: win("e1", "files") }, { "e1-pane": { openTabs: [], activePath: null } })).toBeNull();
    });

    it("still names the window itself for every other role", () => {
        const ref = activeTabRef(session({ activeWindowId: "t1" }), { t1: win("t1", "term") }, {});

        expect(ref && tabRefKey(ref)).toBe("window:t1");
    });
});
