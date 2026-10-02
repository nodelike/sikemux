import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { computeGraph, GitGraph, withoutScope } from "./GitGraph";
import type { GitCommit } from "../api/git";

afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
});

const node = (hash: string, parents: string[], extra: Partial<GitCommit> = {}): GitCommit => ({
    hash,
    full_hash: `full-${hash}`,
    parents: parents.map((p) => `full-${p}`),
    author: "sikemux",
    author_email: "sikemux@example.test",
    date: "1m ago",
    subject: `commit ${hash}`,
    refs: [],
    unpushed: false,
    ...extra,
});

/** A straight chain, newest first: c0 is the child of c1, and so on. */
function chain(count: number): GitCommit[] {
    return Array.from({ length: count }, (_, i) => ({
        hash: `c${i}`,
        full_hash: `full-c${i}`,
        parents: i + 1 < count ? [`full-c${i + 1}`] : [],
        author: "sikemux",
        author_email: "sikemux@example.test",
        date: "1m ago",
        subject: `commit ${i}`,
        refs: i === 0 ? ["HEAD"] : [],
        unpushed: false,
    }));
}

describe("computeGraph", () => {
    it("draws a straight chain in one lane", () => {
        const { rows, maxLanes } = computeGraph(chain(6));
        expect(maxLanes).toBe(1);
        expect(rows.every((r) => r.lane === 0 && r.through.length === 0)).toBe(true);
    });

    it("closes the lane when a parent arrives before its own child", () => {
        const commits = chain(6);
        [commits[2], commits[3]] = [commits[3], commits[2]];

        const { rows } = computeGraph(commits);

        expect(rows.slice(4).every((r) => r.through.length === 0)).toBe(true);
    });
});

describe("computeGraph merges", () => {
    it("gives a merged branch its own lane and brings it back into the first", () => {
        const { rows, maxLanes } = computeGraph([node("m", ["a", "b"]), node("b", ["a"]), node("a", [])]);
        expect(maxLanes).toBe(2);
        expect(rows[0].branches.map((b) => b.toLane)).toEqual([0, 1]);
        expect(rows[1]).toMatchObject({ lane: 1, through: [{ lane: 0 }], branches: [{ toLane: 0 }] });
        expect(rows[2].merges.map((m) => m.fromLane)).toEqual([0]);
    });

    it("joins a second parent to the lane already waiting for it", () => {
        const { rows } = computeGraph([node("c1", ["a"]), node("c2", ["b", "a"], { unpushed: true }), node("b", ["a"]), node("a", [])]);
        expect(rows[1]).toMatchObject({ lane: 1, unpushed: true });
        expect(rows[1].branches).toEqual([
            { toLane: 1, lane: 1, color: 1, unpushed: true },
            { toLane: 0, lane: 0, color: 0, unpushed: true },
        ]);
        expect(rows[2].merges[0]).toMatchObject({ fromLane: 1, unpushed: true });
    });

    it("ignores parents outside the loaded history", () => {
        const { rows } = computeGraph([node("x", ["gone"])]);
        expect(rows[0].branches).toEqual([]);
    });
});

describe("withoutScope", () => {
    it("drops a conventional-commit prefix but keeps a subject that is only a prefix", () => {
        expect(withoutScope("feat(git)!: draw lanes")).toBe("draw lanes");
        expect(withoutScope("fix: typo")).toBe("typo");
        expect(withoutScope("Merge branch x")).toBe("Merge branch x");
    });
});

describe("GitGraph", () => {
    const history = [
        node("m", ["a", "b"], { refs: ["HEAD -> main", "origin/main", "tag: v1", "HEAD"], subject: "feat(git): merge" }),
        node("b", ["a"], { unpushed: true }),
        node("a", []),
    ];

    it("labels a commit's refs by kind, folding the overflow into a count", () => {
        render(<GitGraph commits={history} selectedIndex={0} focused range={null} onSelect={() => {}} onActivate={() => {}} />);
        const row = screen.getByRole("button", { name: "m feat(git): merge" });
        expect(row).toHaveClass("sel");
        expect(row.querySelector(".gg-ref.branch")).toHaveTextContent("main");
        expect(row.querySelector(".gg-ref.remote")).toHaveTextContent("origin/main");
        expect(row.querySelector(".gg-ref.more")).toHaveTextContent("+1");
        expect(row.querySelector(".gg-ref.more")).toHaveAttribute("title", "tag: v1");
        expect(row).toHaveTextContent("merge");
        expect(row.querySelector(".gg-subj")).toHaveTextContent(/^merge$/);
    });

    it("shows a tag badge", () => {
        const commits = [node("t", [], { refs: ["tag: v2", "HEAD"] })];
        render(<GitGraph commits={commits} selectedIndex={0} focused={false} range={null} onSelect={() => {}} onActivate={() => {}} />);
        const row = screen.getByRole("button", { name: "t commit t" });
        expect(row).not.toHaveClass("sel");
        expect(row.querySelector(".gg-ref.tag")).toHaveTextContent("v2");
    });

    it("marks the rows inside a range", () => {
        render(<GitGraph commits={history} selectedIndex={0} focused range={[1, 2]} onSelect={() => {}} onActivate={() => {}} />);
        expect(screen.getAllByRole("button").map((row) => row.classList.contains("ranged"))).toEqual([false, true, true]);
    });

    it("moves the selection with the arrows, Home and End, and opens a commit with Enter or a double-click", () => {
        const onSelect = vi.fn();
        const onActivate = vi.fn();
        render(<GitGraph commits={history} selectedIndex={0} focused range={null} onSelect={onSelect} onActivate={onActivate} />);
        const rows = screen.getAllByRole("button");

        fireEvent.keyDown(rows[0], { key: "ArrowDown" });
        expect(onSelect).toHaveBeenLastCalledWith(1);
        expect(rows[1]).toHaveFocus();
        fireEvent.keyDown(rows[0], { key: "ArrowUp" });
        expect(onSelect).toHaveBeenLastCalledWith(0);
        fireEvent.keyDown(rows[0], { key: "End" });
        expect(onSelect).toHaveBeenLastCalledWith(2);
        fireEvent.keyDown(rows[2], { key: "ArrowDown" });
        expect(onSelect).toHaveBeenLastCalledWith(2);
        fireEvent.keyDown(rows[2], { key: "Home" });
        expect(onSelect).toHaveBeenLastCalledWith(0);

        fireEvent.keyDown(rows[1], { key: "Enter" });
        expect(onSelect).toHaveBeenLastCalledWith(1);
        expect(onActivate).toHaveBeenCalledOnce();
        fireEvent.click(rows[2]);
        expect(onSelect).toHaveBeenLastCalledWith(2);
        fireEvent.doubleClick(rows[2]);
        expect(onActivate).toHaveBeenCalledTimes(2);
    });

    it("says so when there are no commits", () => {
        render(<GitGraph commits={[]} selectedIndex={0} focused range={null} onSelect={() => {}} onActivate={() => {}} />);
        expect(screen.getByText("no commits")).toBeInTheDocument();
    });

    it("paints unpushed commits in the warning colour, rings HEAD and curves lines that change lane", () => {
        const fills: string[] = [];
        const ctx = {
            fillStyle: "",
            strokeStyle: "",
            setTransform: vi.fn(),
            clearRect: vi.fn(),
            beginPath: vi.fn(),
            moveTo: vi.fn(),
            lineTo: vi.fn(),
            bezierCurveTo: vi.fn(),
            stroke: vi.fn(),
            arc: vi.fn(),
            fill: vi.fn(() => fills.push(ctx.fillStyle)),
        };
        vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(ctx as never);
        const commits = [node("m", ["a", "b"], { refs: ["HEAD"] }), node("b", ["a"], { unpushed: true }), node("a", [])];
        render(<GitGraph commits={commits} selectedIndex={1} focused range={null} onSelect={() => {}} onActivate={() => {}} />);

        expect(ctx.bezierCurveTo).toHaveBeenCalled();
        expect(fills).toContain("#ffca85");
        const radii = ctx.arc.mock.calls.map((call) => call[2]);
        expect(radii).toContain(5);
        expect(radii).toContain(7);
    });
});
