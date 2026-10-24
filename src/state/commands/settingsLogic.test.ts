import { describe, expect, it } from "vitest";
import { mergePinnedIntoRoots, normaliseProjectRoots } from "./settingsLogic";

describe("settings command logic", () => {
    it("normalises project roots from legacy strings and objects", () => {
        expect(
            normaliseProjectRoots([
                "/repo/a",
                { path: "/repo/b", depth: 2.6 },
                { path: "/repo/c", depth: -3 },
                { path: "/repo/d", depth: Number.NaN },
                { path: "/repo/e", depth: 1, selfIndex: true },
                { nope: true },
                null,
            ]),
        ).toEqual([
            { path: "/repo/a", depth: 1, selfIndex: false },
            { path: "/repo/b", depth: 3, selfIndex: false },
            { path: "/repo/c", depth: 0, selfIndex: false },
            { path: "/repo/d", depth: 1, selfIndex: false },
            { path: "/repo/e", depth: 1, selfIndex: true },
        ]);
        expect(normaliseProjectRoots("bad")).toEqual([]);
    });

    it("folds a legacy pinned list into roots without losing either", () => {
        const roots = [{ path: "/repo/shared", depth: 2, selfIndex: false }];
        expect(mergePinnedIntoRoots(roots, ["/repo/scratch", { path: "/repo/shared" }, { nope: true }, 42])).toEqual([
            // An entry that was both a root and pinned keeps its depth and
            // gains self-indexing, rather than appearing twice.
            { path: "/repo/shared", depth: 2, selfIndex: true },
            { path: "/repo/scratch", depth: 0, selfIndex: true },
        ]);
    });

    it("leaves roots alone when nothing was pinned", () => {
        const roots = [{ path: "/repo/a", depth: 1, selfIndex: false }];
        expect(mergePinnedIntoRoots(roots, undefined)).toEqual(roots);
    });
});
