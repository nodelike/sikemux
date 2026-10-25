import { describe, expect, it } from "vitest";
import { collapseDiff, fencedDiff } from "./diff";
import type { DiffLine } from "./types";

const same = (text: string): DiffLine => ({ sign: " ", text });

describe("fencedDiff", () => {
    it("reads a patch the agent wrote in a fence", () => {
        const lines = fencedDiff(" .stage {\n-    background: var(--pane);\n+    background: transparent;\n }\n");
        expect(lines?.map((line) => line.sign)).toEqual([" ", "-", "+", " "]);
        expect(lines?.[1].text).toBe("    background: var(--pane);");
        expect(lines?.[2].text.slice(...(lines[2].mark ?? [0, 0]))).toBe("transparent");
    });

    it("takes the fence's word for it when it says diff", () => {
        expect(fencedDiff("+added line\n+another\n", "diff")?.map((line) => line.sign)).toEqual(["+", "+"]);
    });

    it("keeps a hunk header as a plain row", () => {
        const lines = fencedDiff("@@ -1,2 +1,2 @@\n-old\n+new\n", "patch");
        expect(lines?.[0]).toEqual({ sign: " ", text: "@@ -1,2 +1,2 @@" });
    });

    it("says nothing for a fence too short or long to be a patch, or a diff fence with no changes", () => {
        expect(fencedDiff("+one\n", "diff")).toBeNull();
        expect(fencedDiff(Array.from({ length: 401 }, () => "+x").join("\n"), "diff")).toBeNull();
        expect(fencedDiff(" context\n more context\n", "diff")).toBeNull();
    });

    it("leaves ordinary output alone", () => {
        expect(fencedDiff("900x600 → 0.4 fps ok\n1024x600 → 60.4 fps FLOOD\n")).toBeNull();
        expect(fencedDiff("const x = 1;\nconst y = x - 2;\n")).toBeNull();
        expect(fencedDiff("rm -rf build\npnpm install\n")).toBeNull();
    });
});

describe("collapseDiff", () => {
    it("folds the unchanged stretches away and counts them", () => {
        const lines: DiffLine[] = [
            ...Array.from({ length: 10 }, (_, index) => same(`line ${index}`)),
            { sign: "-", text: "line 10" },
            { sign: "+", text: "changed" },
            ...Array.from({ length: 9 }, (_, index) => same(`line ${index + 11}`)),
        ];
        const view = collapseDiff(lines, 2);
        expect(view.hidden).toBe(15);
        expect(view.rows.filter((row) => "gap" in row)).toHaveLength(2);
        expect(view.rows.filter((row) => "sign" in row && row.sign !== " ")).toHaveLength(2);
    });

    it("keeps a short diff whole", () => {
        const view = collapseDiff([same("a"), { sign: "-", text: "b" }, { sign: "+", text: "B" }], 3);
        expect(view.hidden).toBe(0);
        expect(view.rows.every((row) => "sign" in row)).toBe(true);
    });
});
