import { describe, expect, it } from "vitest";
import { patchToRows } from "./patchRows";

describe("patchToRows", () => {
    it("numbers added and unchanged lines in the new file and removed ones in the old", () => {
        const patch = "@@ -3,3 +3,3 @@\n keep\n-old\n+new\n tail";
        expect(patchToRows(patch)).toEqual([
            [3, 2, ""],
            [0, 3, "keep"],
            [2, 4, "old"],
            [1, 4, "new"],
            [0, 5, "tail"],
        ]);
    });

    it("counts the unchanged lines between hunks, which the patch leaves out", () => {
        const patch = "@@ -1,2 +1,2 @@\n a\n-b\n+B\n@@ -10,1 +10,2 @@\n j\n+k";
        const rows = patchToRows(patch);
        expect(rows[3]).toEqual([3, 7, ""]);
        expect(rows.slice(4)).toEqual([
            [0, 10, "j"],
            [1, 11, "k"],
        ]);
    });

    it("reads a new file, with nothing hidden before it", () => {
        expect(patchToRows("@@ -0,0 +1,2 @@\n+one\n+two\n")).toEqual([
            [1, 1, "one"],
            [1, 2, "two"],
        ]);
    });

    it("skips the note about a missing newline at the end", () => {
        expect(patchToRows("@@ -1 +1 @@\n-a\n\\ No newline at end of file\n+b")).toEqual([
            [2, 1, "a"],
            [1, 1, "b"],
        ]);
    });
});
