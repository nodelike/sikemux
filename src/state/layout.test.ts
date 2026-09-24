import { describe, expect, it } from "vitest";
import { MIN_FRAC, collectPanes, computeLayout, makePane, neighborPane, removePane, resizeTowards, setSplitSizes, splitPane } from "./layout";
import type { LayoutNode, PaneNode } from "./types";

const pane = (id: string): PaneNode => ({ type: "pane", id, cwd: `/tmp/${id}`, kind: "terminal", title: id });

describe("layout helpers", () => {
    it("creates panes with stable defaults", () => {
        const p = makePane("/repo");
        expect(p.type).toBe("pane");
        expect(p.id).toMatch(/^pane-/);
        expect(p.cwd).toBe("/repo");
        expect(p.kind).toBe("terminal");
        expect(p.title).toBe("shell");

        expect(makePane("/repo", { kind: "editor" }).title).toBe("editor");
        expect(makePane("/repo", { kind: "sikemux.example:view" }).title).toBe("sikemux.example:view");
        expect(makePane("/repo", { startup: "top" }).title).toBe("top");
    });

    it("collects panes from nested trees in render order", () => {
        const tree: LayoutNode = {
            type: "split",
            id: "s1",
            dir: "row",
            sizes: [0.6, 0.4],
            children: [pane("a"), { type: "split", id: "s2", dir: "column", sizes: [0.5, 0.5], children: [pane("b"), pane("c")] }],
        };

        expect(collectPanes(tree).map((p) => p.id)).toEqual(["a", "b", "c"]);
    });

    it("splits a pane into a new split", () => {
        const root = pane("a");
        const next = splitPane(root, "a", "row", pane("b"));

        expect(next.type).toBe("split");
        if (next.type !== "split") throw new Error("expected split");
        expect(next.dir).toBe("row");
        expect(next.sizes).toEqual([0.5, 0.5]);
        expect(next.children.map((c) => c.id)).toEqual(["a", "b"]);
    });

    it("adds a pane beside an existing same-axis split and halves the target pane", () => {
        const root: LayoutNode = { type: "split", id: "s1", dir: "row", sizes: [0.7, 0.3], children: [pane("a"), pane("c")] };
        const next = splitPane(root, "a", "row", pane("b"));

        expect(next.type).toBe("split");
        if (next.type !== "split") throw new Error("expected split");
        expect(next.children.map((c) => c.id)).toEqual(["a", "b", "c"]);
        expect(next.sizes).toEqual([0.35, 0.35, 0.3]);
    });

    it("removes panes and collapses single-child splits", () => {
        const root: LayoutNode = { type: "split", id: "s1", dir: "row", sizes: [0.5, 0.5], children: [pane("a"), pane("b")] };

        expect(removePane(root, "missing")).toEqual(root);
        expect(removePane(root, "a")).toEqual(pane("b"));
        expect(removePane(pane("a"), "a")).toBeNull();
    });

    it("computes row and column rectangles", () => {
        const root: LayoutNode = {
            type: "split",
            id: "s1",
            dir: "row",
            sizes: [0.25, 0.75],
            children: [pane("left"), { type: "split", id: "s2", dir: "column", sizes: [0.4, 0.6], children: [pane("top"), pane("bottom")] }],
        };

        const { panes, dividers } = computeLayout(root);
        expect(panes.get("left")).toEqual({ x: 0, y: 0, w: 0.25, h: 1 });
        expect(panes.get("top")).toEqual({ x: 0.25, y: 0, w: 0.75, h: 0.4 });
        expect(panes.get("bottom")).toEqual({ x: 0.25, y: 0.4, w: 0.75, h: 0.6 });
        expect(dividers).toHaveLength(2);
    });

    it("finds directional neighbours by geometry", () => {
        const root: LayoutNode = {
            type: "split",
            id: "s1",
            dir: "row",
            sizes: [0.5, 0.5],
            children: [pane("left"), { type: "split", id: "s2", dir: "column", sizes: [0.5, 0.5], children: [pane("top"), pane("bottom")] }],
        };
        const { panes } = computeLayout(root);

        expect(neighborPane(panes, "left", "right")).toBe("top");
        expect(neighborPane(panes, "top", "left")).toBe("left");
        expect(neighborPane(panes, "bottom", "up")).toBe("top");
        expect(neighborPane(panes, "top", "down")).toBe("bottom");
    });

    it("updates split sizes and resizes without violating minimum pane size", () => {
        const root: LayoutNode = { type: "split", id: "s1", dir: "row", sizes: [0.5, 0.5], children: [pane("a"), pane("b")] };
        expect(setSplitSizes(root, "s1", [0.25, 0.75])).toMatchObject({ sizes: [0.25, 0.75] });

        const resized = resizeTowards(root, "a", "right", 0.7);
        expect(resized.type).toBe("split");
        if (resized.type !== "split") throw new Error("expected split");
        expect(resized.sizes[0]).toBeCloseTo(1 - MIN_FRAC);
        expect(resized.sizes[1]).toBeCloseTo(MIN_FRAC);
    });
});

describe("stack splits", () => {
    const stack = (children: LayoutNode[]): LayoutNode => ({ type: "split", id: "s", dir: "stack", children, sizes: [0.5, 0.5] });

    it("gives the whole rect to the pane holding focus and covers the rest", () => {
        const { panes, stacked } = computeLayout(stack([pane("a"), pane("b")]), "b");

        expect(panes.get("b")).toEqual({ x: 0, y: 0, w: 1, h: 1 });
        expect(panes.has("a")).toBe(false);
        expect(stacked.get("a")).toEqual({ x: 0, y: 0, w: 1, h: 1 });
    });

    /*
     * A covered pane keeps the stack's size rather than none, so a terminal
     * coming back to the top does not briefly believe it is zero-sized and
     * reflow its scrollback.
     */
    it("measures a covered pane at the stack's rect, not at nothing", () => {
        const layout: LayoutNode = {
            type: "split",
            id: "row",
            dir: "row",
            children: [pane("left"), stack([pane("a"), pane("b")])],
            sizes: [0.25, 0.75],
        };
        const { stacked } = computeLayout(layout, "a");

        expect(stacked.get("b")).toEqual({ x: 0.25, y: 0, w: 0.75, h: 1 });
    });

    it("shows the first pane when focus is elsewhere", () => {
        const { panes } = computeLayout(stack([pane("a"), pane("b")]), "somewhere-else");

        expect(panes.has("a")).toBe(true);
        expect(panes.has("b")).toBe(false);
    });

    it("reports a strip naming every tab and the one on top", () => {
        const { stacks, inStack } = computeLayout(stack([pane("a"), pane("b")]), "b");

        expect(stacks).toHaveLength(1);
        expect(stacks[0].tabs.map((t) => t.id)).toEqual(["a", "b"]);
        expect(stacks[0].activePaneId).toBe("b");
        expect(stacks[0].rect).toEqual({ x: 0, y: 0, w: 1, h: 1 });
        expect([...inStack].sort()).toEqual(["a", "b"]);
    });

    /*
     * A tab may be a whole split rather than a single pane, so the strip speaks
     * for it with the first pane inside it.
     */
    it("names a tab by the first pane inside it when that tab is itself a split", () => {
        const nested: LayoutNode = { type: "split", id: "row", dir: "row", children: [pane("b1"), pane("b2")], sizes: [0.5, 0.5] };
        const { stacks, inStack } = computeLayout(stack([pane("a"), nested]), "b2");

        expect(stacks[0].tabs.map((t) => t.id)).toEqual(["a", "b1"]);
        expect(stacks[0].activePaneId).toBe("b1");
        expect([...inStack].sort()).toEqual(["a", "b1", "b2"]);
    });

    it("reports no strips for a layout with no stack in it", () => {
        expect(computeLayout(pane("solo"), "solo").stacks).toEqual([]);
    });

    /* Nothing sits between stacked panes, so there is no edge to drag. */
    it("draws no dividers", () => {
        expect(computeLayout(stack([pane("a"), pane("b")]), "a").dividers).toEqual([]);
    });

    /* A stack has no axis to give space along, so resizing walks past it. */
    it("is skipped when resizing towards a neighbour", () => {
        const layout = stack([pane("a"), pane("b")]);

        expect(resizeTowards(layout, "a", "right")).toBe(layout);
    });
});
