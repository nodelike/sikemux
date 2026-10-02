import { describe, expect, it } from "vitest";
import { worktreeSwitchState } from "./worktreeSwitch";

const worktree = { repo: "/code/sikemux", path: "/code/sikemux.worktrees/fix-pty", branch: "sikemux/fix-pty", base: "main", startSha: "abc" };

describe("worktreeSwitchState", () => {
    const base = { worktree: undefined, isRepo: true, started: false, preparing: null, on: false };

    it("offers the choice on a fresh chat in a repository", () => {
        expect(worktreeSwitchState(base)).toEqual({ kind: "choosing", on: false });
        expect(worktreeSwitchState({ ...base, on: true })).toEqual({ kind: "choosing", on: true });
    });

    it("hides outside a repository and once the chat has started", () => {
        expect(worktreeSwitchState({ ...base, isRepo: false })).toEqual({ kind: "hidden" });
        expect(worktreeSwitchState({ ...base, isRepo: null })).toEqual({ kind: "hidden" });
        expect(worktreeSwitchState({ ...base, started: true })).toEqual({ kind: "hidden" });
    });

    it("reports the setup step, then the branch the agent works on", () => {
        expect(worktreeSwitchState({ ...base, on: true, preparing: "Creating worktree" })).toEqual({ kind: "preparing", step: "Creating worktree" });
        expect(worktreeSwitchState({ ...base, worktree, preparing: "Running Install" })).toEqual({ kind: "preparing", step: "Running Install" });
        expect(worktreeSwitchState({ ...base, started: true, worktree })).toEqual({ kind: "in", branch: "sikemux/fix-pty", path: worktree.path });
    });
});
