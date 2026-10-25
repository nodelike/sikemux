import { describe, expect, it } from "vitest";
import { SUBSEQ_BASE, fuzzyScore, isSubstringMatch, rankBy } from "./fuzzy";

describe("fuzzy matching", () => {
    it("bounded ranking preserves full ranking, substring preference, and stable ties", () => {
        const files = Array.from({ length: 10_000 }, (_, index) => ({
            id: index,
            name: `${index % 3 === 0 ? "component" : "cmpnt"}-${index % 97}.ts`,
            path: `src/project-${index % 53}/component-${index}.ts`,
        }));
        const fields = (file: (typeof files)[number]) => [file.name, file.path];
        for (const query of ["", "component", "cmp", "cpnt", "42", "no match"]) {
            const all = rankBy(query, files, fields);
            for (const limit of [0, 1, 7, 200, 12_000]) {
                expect(rankBy(query, files, fields, limit)).toEqual(all.slice(0, limit));
            }
        }
        expect(rankBy("ab", ["a_b", "axb", "ab"], String, 1)).toEqual(["ab"]);
    });

    it("returns zero for empty queries", () => {
        expect(fuzzyScore("", "anything")).toBe(0);
        expect(rankBy("", [3, 1, 2], String)).toEqual([3, 1, 2]);
    });

    it("scores direct substring matches ahead of subsequence matches", () => {
        const substring = fuzzyScore("git", "git pane");
        const subsequence = fuzzyScore("gp", "git pane");

        expect(isSubstringMatch(substring)).toBe(true);
        expect(subsequence).toBeGreaterThanOrEqual(SUBSEQ_BASE);
        expect(substring).toBeLessThan(subsequence);
    });

    it("prefers word-boundary matches over mid-word matches", () => {
        expect(fuzzyScore("bar", "foo bar")).toBeLessThan(fuzzyScore("bar", "foobarbaz"));
    });

    it("nudges ties toward earlier fields", () => {
        expect(fuzzyScore("abc", "zzz", "abc")).toBeGreaterThan(fuzzyScore("abc", "abc", "zzz"));
    });

    it("ranks objects by matching fields and drops non-matches", () => {
        const rows = [
            { name: "project-git-view", tag: "screenshot" },
            { name: "terminal", tag: "pty shell" },
            { name: "bruno", tag: "api" },
        ];

        expect(rankBy("git", rows, (r) => [r.name, r.tag])).toEqual([rows[0]]);
        expect(rankBy("api", rows, (r) => [r.name, r.tag])).toEqual([rows[2]]);
    });
});
