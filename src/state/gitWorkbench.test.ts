import { beforeEach, describe, expect, it, vi } from "vitest";
import { commitGitDraft, generateGitDraft, runRepositoryGit, setGitDraft, useGitWorkbench } from "./gitWorkbench";
import { git } from "../api/git";
vi.mock("../api/git", () => ({ git: { status: vi.fn(), commit: vi.fn(), aiMessage: vi.fn() } }));
vi.mock("./git", () => ({ runGitCmd: (_label: string, action: () => Promise<unknown>) => action() }));
beforeEach(() => {
    vi.clearAllMocks();
    useGitWorkbench.setState({ drafts: {}, operations: {}, provider: "codex", model: "test-model" });
});
describe("repository Git work", () => {
    it("isolates drafts and preserves edits made during a commit", async () => {
        setGitDraft("/a", "first");
        setGitDraft("/b", "second");
        vi.mocked(git.status).mockResolvedValue({
            branch: "main",
            upstream: null,
            ahead: 0,
            behind: 0,
            files: [{ path: "a", index: "M", worktree: " " }],
        });
        let finish!: (value: string) => void;
        vi.mocked(git.commit).mockImplementation(
            () =>
                new Promise((resolve) => {
                    finish = resolve;
                }),
        );
        const committing = commitGitDraft("/a");
        await vi.waitFor(() => expect(git.commit).toHaveBeenCalledWith("/a", "first"));
        setGitDraft("/a", "next commit");
        finish("done");
        await committing;
        expect(useGitWorkbench.getState().drafts).toEqual({ "/a": "next commit", "/b": "second" });
    });
    it("keeps drafts on failure and never stages everything implicitly", async () => {
        setGitDraft("/a", "keep me");
        vi.mocked(git.status).mockResolvedValue({
            branch: "main",
            upstream: null,
            ahead: 0,
            behind: 0,
            files: [{ path: "a", index: " ", worktree: "M" }],
        });
        expect(await commitGitDraft("/a")).toBe(false);
        expect(git.commit).not.toHaveBeenCalled();
        expect(useGitWorkbench.getState().drafts["/a"]).toBe("keep me");
        expect(useGitWorkbench.getState().operations["/a"].error).toContain("Stage");
    });
    it("keeps a rejected commit draft and clears only a successful one", async () => {
        setGitDraft("/a", "keep me");
        vi.mocked(git.status).mockResolvedValue({
            branch: "main",
            upstream: null,
            ahead: 0,
            behind: 0,
            files: [{ path: "a", index: "M", worktree: " " }],
        });
        vi.mocked(git.commit).mockRejectedValueOnce(new Error("hook rejected"));
        expect(await commitGitDraft("/a")).toBe(false);
        expect(useGitWorkbench.getState().drafts["/a"]).toBe("keep me");
        vi.mocked(git.commit).mockResolvedValueOnce("done");
        expect(await commitGitDraft("/a")).toBe(true);
        expect(useGitWorkbench.getState().drafts["/a"]).toBe("");
    });
    it("uses the chosen provider and preserves drafts on generation failure", async () => {
        setGitDraft("/a", "written by me");
        vi.mocked(git.aiMessage).mockRejectedValueOnce(new Error("CLI unavailable"));
        expect(await generateGitDraft("/a")).toBe(false);
        expect(git.aiMessage).toHaveBeenCalledWith("/a", "codex", "test-model", expect.any(Function));
        expect(useGitWorkbench.getState().drafts["/a"]).toBe("written by me");
    });
    it("locks a repository while allowing independent repositories to work", async () => {
        let finish!: () => void;
        const action = vi.fn(
            () =>
                new Promise<void>((resolve) => {
                    finish = resolve;
                }),
        );
        const first = runRepositoryGit("/a", "Push", action);
        expect(await runRepositoryGit("/a", "Push", action)).toBe(false);
        expect(await runRepositoryGit("/b", "Fetch", async () => {})).toBe(true);
        expect(action).toHaveBeenCalledTimes(1);
        finish();
        await first;
    });
});
