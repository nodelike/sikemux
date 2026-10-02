import { describe, expect, it } from "vitest";
import { gitFileBadges, gitFileDecoration, gitStatusDecoration } from "./gitFileStatus";

describe("git file status decorations", () => {
    it.each([
        ["?", "U", "u"],
        ["U", "U", "u"],
        ["A", "A", "a"],
        ["D", "D", "d"],
        ["R", "R", "r"],
        ["C", "C", "r"],
        ["T", "T", "m"],
        ["M", "M", "m"],
    ])("maps %s to the existing %s decoration", (raw, letter, cls) => {
        expect(gitStatusDecoration(raw)).toMatchObject({ letter, cls });
    });

    it("keeps the file-tree conflict and deletion priority", () => {
        expect(gitFileDecoration({ path: "file.ts", index: "A", worktree: "D" })).toMatchObject({ letter: "D", cls: "d" });
        expect(gitFileDecoration({ path: "file.ts", index: "M", worktree: "?" })).toMatchObject({ letter: "U", cls: "u" });
    });

    it("shows one badge for untracked and unmerged files", () => {
        expect(gitFileBadges({ path: "file.ts", index: "?", worktree: "?" })).toEqual([
            { letter: "U", cls: "u", label: "untracked", source: "untracked" },
        ]);
        expect(gitFileBadges({ path: "file.ts", index: "U", worktree: "U" })).toEqual([
            { letter: "U", cls: "u", label: "unmerged", source: "unmerged" },
        ]);
    });

    it("shows a badge per side when the index and the working tree differ", () => {
        expect(gitFileBadges({ path: "file.ts", index: "R", worktree: "M" })).toMatchObject([
            { letter: "R", source: "staged" },
            { letter: "M", source: "unstaged" },
        ]);
        expect(gitFileBadges({ path: "file.ts", index: " ", worktree: "M" })).toMatchObject([{ letter: "M", source: "unstaged" }]);
    });

    it("ranks additions and renames above plain edits, and falls back to modified", () => {
        expect(gitFileDecoration({ path: "file.ts", index: "M", worktree: "A" })).toMatchObject({ letter: "A", cls: "a" });
        expect(gitFileDecoration({ path: "file.ts", index: "R", worktree: "M" })).toMatchObject({ letter: "R", label: "renamed" });
        expect(gitFileDecoration({ path: "file.ts", index: "X", worktree: " " })).toMatchObject({ letter: "X", cls: "m", label: "X" });
        expect(gitFileDecoration({ path: "file.ts", index: " ", worktree: " " })).toEqual({ letter: "M", cls: "m", label: "modified" });
        expect(gitStatusDecoration("  ")).toBeNull();
    });
});
