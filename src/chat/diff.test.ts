import { describe, expect, it } from "vitest";
import { collapseDiff, diffLines, fencedDiff, toolDiff, toolFailure } from "./diff";
import type { AcpToolCall } from "./types";

const tool = (patch: Partial<AcpToolCall>): AcpToolCall => ({ toolCallId: "tool-1", title: "Edit", kind: "edit", ...patch });

describe("diffLines", () => {
    it("numbers both sides and keeps what did not change", () => {
        const lines = diffLines("a\nb\nc\n", "a\nB\nc\n");
        expect(lines?.map((line) => `${line.sign}${line.text}`)).toEqual([" a", "-b", "+B", " c"]);
        expect(lines?.[0]).toMatchObject({ oldLine: 1, newLine: 1 });
        expect(lines?.[1]).toMatchObject({ oldLine: 2 });
        expect(lines?.[2]).toMatchObject({ newLine: 2 });
        expect(lines?.[3]).toMatchObject({ oldLine: 3, newLine: 3 });
    });

    it("marks the span that changed when one line replaces one line", () => {
        const lines = diffLines("    background: var(--pane);\n", "    background: transparent;\n");
        const deleted = lines?.find((line) => line.sign === "-");
        const added = lines?.find((line) => line.sign === "+");
        expect(deleted?.text.slice(...(deleted?.mark ?? [0, 0]))).toBe("var(--pane)");
        expect(added?.text.slice(...(added?.mark ?? [0, 0]))).toBe("transparent");
    });

    it("leaves two unrelated lines unmarked rather than inventing a span", () => {
        const lines = diffLines("alpha\n", "beta\n");
        expect(lines?.every((line) => line.mark === undefined)).toBe(true);
    });

    it("reads an insertion as added lines, not as a rewrite", () => {
        const lines = diffLines("one\ntwo\n", "one\nextra\ntwo\n");
        expect(lines?.map((line) => `${line.sign}${line.text}`)).toEqual([" one", "+extra", " two"]);
    });

    it("gives up on a change too large to read in a transcript", () => {
        const big = Array.from({ length: 900 }, (_, index) => `line ${index}`).join("\n");
        expect(diffLines(big, `${big}\nmore`)).toBeNull();
    });

    it("keeps a line both sides share in the middle of a rewrite", () => {
        const lines = diffLines("head\na\nkeep\nb\ntail\n", "head\nx\nkeep\ny\nz\ntail\n");
        expect(lines?.map((line) => `${line.sign}${line.text}`)).toEqual([" head", "-a", "+x", " keep", "-b", "+y", "+z", " tail"]);
        expect(lines?.find((line) => line.text === "keep")).toMatchObject({ oldLine: 3, newLine: 3 });
    });

    it("reads a line moved up past another as one added and one deleted", () => {
        const lines = diffLines("a\nb\nc\n", "c\na\nb\n");
        expect(lines?.map((line) => `${line.sign}${line.text}`)).toEqual(["+c", " a", " b", "-c"]);
    });

    it("marks nothing inside a block of several lines replaced by several", () => {
        const lines = diffLines("const first = 1;\nconst second = 2;\n", "const first = 10;\nconst second = 20;\n");
        expect(lines?.map((line) => line.sign)).toEqual(["-", "-", "+", "+"]);
        expect(lines?.every((line) => line.mark === undefined)).toBe(true);
    });

    it("does not mark a very long line", () => {
        const long = "x".repeat(250);
        const lines = diffLines(`${long}a\n`, `${long}b\n`);
        expect(lines?.every((line) => line.mark === undefined)).toBe(true);
    });

    it("reads an empty file emptied as no lines at all", () => {
        expect(diffLines("", "")).toEqual([]);
        expect(diffLines("", "only\n")?.map((line) => line.sign)).toEqual(["+"]);
    });
});

describe("toolDiff", () => {
    it("takes the change the adapter already computed", () => {
        const diff = toolDiff(
            tool({
                content: [
                    { type: "diff", path: "src/styles/stage.css", oldText: "background: var(--pane);\n", newText: "background: transparent;\n" },
                ],
            }),
        );
        expect(diff).toMatchObject({ path: "src/styles/stage.css", adds: 1, dels: 1 });
    });

    it("falls back to what the tool was asked to do", () => {
        const diff = toolDiff(tool({ rawInput: { file_path: "src/app.ts", old_string: "a\n", new_string: "a\nb\n" } }));
        expect(diff).toMatchObject({ path: "src/app.ts", adds: 1, dels: 0 });
    });

    it("reads a write of a new file as all additions", () => {
        const diff = toolDiff(tool({ rawInput: { file_path: "src/new.ts", content: "one\ntwo\n" } }));
        expect(diff).toMatchObject({ path: "src/new.ts", adds: 2, dels: 0 });
    });

    it("finds a diff wrapped in a content block and skips the blocks that are not one", () => {
        const diff = toolDiff(
            tool({
                content: [
                    "stray",
                    { type: "content", content: { type: "text", text: "Editing" } },
                    { type: "diff", newText: "no path\n" },
                    { type: "content", content: { type: "diff", path: "src/new.ts", newText: "one\n" } },
                ],
            }),
        );
        expect(diff).toMatchObject({ path: "src/new.ts", adds: 1, dels: 0 });
    });

    it("falls back to the raw input when the content has no diff", () => {
        const diff = toolDiff(tool({ content: [{ type: "text", text: "ok" }], rawInput: { path: "a.ts", oldText: "a\n", newText: "b\n" } }));
        expect(diff).toMatchObject({ path: "a.ts", adds: 1, dels: 1 });
        expect(toolDiff(tool({ rawInput: { filePath: "b.ts", content: "x\n" } }))).toMatchObject({ path: "b.ts", adds: 1 });
    });

    it("has nothing to show when it cannot tell what was written or it is too big to read", () => {
        expect(toolDiff(tool({ rawInput: { file_path: "a.ts", old_string: "a\n" } }))).toBeNull();
        expect(toolDiff(tool({ rawInput: "edit a.ts" }))).toBeNull();
        const big = Array.from({ length: 1300 }, (_, index) => `line ${index}`).join("\n");
        expect(toolDiff(tool({ rawInput: { file_path: "big.ts", content: big } }))).toBeNull();
    });

    it("has nothing to show for a call that changed no file", () => {
        expect(toolDiff(tool({ rawInput: { command: "pnpm test" } }))).toBeNull();
        expect(toolDiff(tool({ rawInput: { file_path: "src/app.ts", old_string: "same\n", new_string: "same\n" } }))).toBeNull();
    });
});

describe("toolFailure", () => {
    it("says nothing for a call that did not fail", () => {
        expect(toolFailure(tool({ status: "completed", rawOutput: "error" }))).toBeNull();
    });

    it("reads a failure from plain text or from whichever field the adapter used", () => {
        expect(toolFailure(tool({ status: "failed", rawOutput: " no such file \n" }))).toBe("no such file");
        expect(toolFailure(tool({ status: "failed", rawOutput: { output: "out", stderr: "err" } }))).toBe("out");
        expect(toolFailure(tool({ status: "failed", rawOutput: { error: "denied" } }))).toBe("denied");
    });

    it("says nothing when the failure left no words behind", () => {
        expect(toolFailure(tool({ status: "failed", rawOutput: "   " }))).toBeNull();
        expect(toolFailure(tool({ status: "failed", rawOutput: { code: 1 } }))).toBeNull();
        expect(toolFailure(tool({ status: "failed" }))).toBeNull();
    });

    it("cuts a long failure short", () => {
        const failure = toolFailure(tool({ status: "failed", rawOutput: "e".repeat(500) }));
        expect(failure).toBe(`${"e".repeat(400)}…`);
    });
});

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
        const lines = diffLines(
            Array.from({ length: 20 }, (_, index) => `line ${index}`).join("\n"),
            [
                ...Array.from({ length: 10 }, (_, index) => `line ${index}`),
                "changed",
                ...Array.from({ length: 9 }, (_, index) => `line ${index + 11}`),
            ].join("\n"),
        );
        const view = collapseDiff(lines ?? [], 2);
        expect(view.hidden).toBe(15);
        expect(view.rows.filter((row) => "gap" in row)).toHaveLength(2);
        expect(view.rows.filter((row) => "sign" in row && row.sign !== " ")).toHaveLength(2);
    });

    it("keeps a short diff whole", () => {
        const view = collapseDiff(diffLines("a\nb\n", "a\nB\n") ?? [], 3);
        expect(view.hidden).toBe(0);
        expect(view.rows.every((row) => "sign" in row)).toBe(true);
    });
});
