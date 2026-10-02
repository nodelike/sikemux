import { describe, expect, it } from "vitest";
import { filterByQuery, isInRange } from "./gitPaneLogic";

describe("GitPane extracted logic", () => {
    it("tells whether a row is inside a range", () => {
        expect(isInRange([2, 5], 1)).toBe(false);
        expect(isInRange([2, 5], 2)).toBe(true);
        expect(isInRange([2, 5], 5)).toBe(true);
        expect(isInRange(null, 3)).toBe(false);
    });

    it("filters rows across multiple fields case-insensitively", () => {
        const rows = [
            { path: "src/components/GitPane.tsx", status: "modified" },
            { path: "README.md", status: "clean" },
            { path: "src/bruno/run.ts", status: "staged" },
        ];

        expect(filterByQuery(rows, "git", (r) => [r.path, r.status])).toEqual([rows[0]]);
        expect(filterByQuery(rows, "STAGED", (r) => [r.path, r.status])).toEqual([rows[2]]);
        expect(filterByQuery(rows, "", (r) => [r.path])).toEqual(rows);
    });

    it("treats a missing field as matching nothing", () => {
        const rows = [
            { subject: "fix", author: null },
            { subject: "feat", author: "Ada" },
        ];
        expect(filterByQuery(rows, "ada", (r) => [r.subject, r.author])).toEqual([rows[1]]);
    });
});
