import { describe, expect, it } from "vitest";
import {
    branchMergedLocally,
    branchSlug,
    cwdInWorktree,
    cwdOutsideWorktree,
    pickWorktreeTarget,
    worktreeCandidates,
    worktreesFolder,
} from "./worktreePlan";

const worktree = { repo: "/code/sikemux", path: "/code/sikemux.worktrees/fix-pty", branch: "sikemux/fix-pty", base: "main", startSha: "abc" };

describe("branchSlug", () => {
    it("kebab-cases the first words of the message", () => {
        expect(branchSlug("Fix the flaky PTY test!")).toBe("fix-the-flaky-pty-test");
    });

    it("stops at a word boundary before forty characters", () => {
        const slug = branchSlug("rewrite the terminal renderer so that it never drops a frame while scrolling fast");
        expect(slug).toBe("rewrite-the-terminal-renderer-so-that-it");
        expect(slug.length).toBeLessThanOrEqual(40);
    });

    it("drops links, accents and punctuation", () => {
        expect(branchSlug("Look at https://github.com/o/r/issues/4 — café crash")).toBe("look-at-cafe-crash");
    });

    it("cuts a single overlong word and falls back when nothing is left", () => {
        expect(branchSlug("a".repeat(60))).toBe("a".repeat(40));
        expect(branchSlug("  🙂 ?? ")).toBe("chat");
    });
});

describe("worktree targets", () => {
    it("live in a folder beside the repository", () => {
        expect(worktreesFolder("/code/sikemux")).toBe("/code/sikemux.worktrees");
        expect(worktreeCandidates("/code/sikemux", "fix-pty", 3)).toEqual([
            { branch: "sikemux/fix-pty", path: "/code/sikemux.worktrees/fix-pty" },
            { branch: "sikemux/fix-pty-2", path: "/code/sikemux.worktrees/fix-pty-2" },
            { branch: "sikemux/fix-pty-3", path: "/code/sikemux.worktrees/fix-pty-3" },
        ]);
    });

    it("skip a name whose branch or folder is already taken", () => {
        const candidates = worktreeCandidates("/code/sikemux", "fix-pty", 4);
        expect(pickWorktreeTarget(candidates, new Set(["sikemux/fix-pty"]), new Set(["/code/sikemux.worktrees/fix-pty-2"]))).toEqual(candidates[2]);
        expect(pickWorktreeTarget(candidates.slice(0, 1), new Set(["sikemux/fix-pty"]), new Set())).toBeNull();
    });

    it("keep the folder the project had open inside the checkout", () => {
        expect(cwdInWorktree("/w/lane", "/code/repo/packages/app", "/code/repo")).toBe("/w/lane/packages/app");
        expect(cwdInWorktree("/w/lane", "/code/repo", "/code/repo")).toBe("/w/lane");
        expect(cwdInWorktree("/w/lane", "/elsewhere", "/code/repo")).toBe("/w/lane");
        expect(cwdOutsideWorktree(worktree, "/code/sikemux.worktrees/fix-pty/src")).toBe("/code/sikemux/src");
        expect(cwdOutsideWorktree(worktree, "/code/sikemux.worktrees/fix-pty")).toBe("/code/sikemux");
    });
});

describe("branchMergedLocally", () => {
    it("needs the branch to have moved and nothing left unmerged", () => {
        expect(branchMergedLocally("abc", "abc", 0)).toBe(false);
        expect(branchMergedLocally("def", "abc", 2)).toBe(false);
        expect(branchMergedLocally("def", "abc", 0)).toBe(true);
        expect(branchMergedLocally(null, "abc", 0)).toBe(false);
    });
});
