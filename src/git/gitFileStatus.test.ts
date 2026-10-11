import { describe, expect, it } from "vitest";
import { gitFileBadges, gitFileDecoration, gitFolderDecorations } from "./gitFileStatus";

describe("git file status decorations", () => {
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
});

describe("git folder decorations", () => {
    const classes = (map: Map<string, { cls: string }>) => Object.fromEntries([...map].map(([folder, decoration]) => [folder, decoration.cls]));

    it("marks every folder above a changed file, not just the file", () => {
        const folders = gitFolderDecorations([{ path: "src/chat/longText.ts", index: " ", worktree: "M" }]);
        expect(classes(folders)).toEqual({ src: "m", "src/chat": "m" });
    });

    it("gives a folder the most telling change inside it", () => {
        const folders = gitFolderDecorations([
            { path: "src/App.tsx", index: " ", worktree: "M" },
            { path: "src/chat/new.ts", index: "?", worktree: "?" },
            { path: "docs/old.md", index: "D", worktree: " " },
            { path: "docs/guide.md", index: " ", worktree: "M" },
        ]);
        expect(classes(folders)).toEqual({ src: "u", "src/chat": "u", docs: "d" });
    });

    it("counts a folder git lists as wholly untracked, and leaves files at the root alone", () => {
        const folders = gitFolderDecorations([
            { path: "src/plugins/linear/", index: "?", worktree: "?" },
            { path: "README.md", index: " ", worktree: "M" },
        ]);
        expect(classes(folders)).toEqual({ src: "u", "src/plugins": "u", "src/plugins/linear": "u" });
    });
});
