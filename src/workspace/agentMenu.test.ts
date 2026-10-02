import { describe, expect, it, vi } from "vitest";
import type { Agent, Session } from "../state/types";
import { agentMenu } from "./agentMenu";

const remove = vi.hoisted(() => vi.fn(async () => {}));
vi.mock("../agents/agentWorktree", () => ({ removeAgentWorktree: remove }));

const session: Session = { id: "s", name: "app", kind: "project", cwd: "/code/app", pinned: false, activeWindowId: "w" };
const agent: Agent = { id: "a", type: "claude", title: "fix", startup: "claude", cwd: "/code/app" };
const hints = { close: "", permissions: "" };

describe("agentMenu", () => {
    it("offers to remove the worktree only for an agent working in one", async () => {
        expect(agentMenu(agent, [], session, hints).some((item) => item.label === "Remove worktree…")).toBe(false);

        const inWorktree = {
            ...agent,
            cwd: "/code/app.worktrees/fix",
            worktree: { repo: "/code/app", path: "/code/app.worktrees/fix", branch: "sikemux/fix", base: "main", startSha: "abc" },
        };
        const item = agentMenu(inWorktree, [], session, hints).find((candidate) => candidate.label === "Remove worktree…");
        expect(item).toBeDefined();
        item?.run?.();
        await vi.waitFor(() => expect(remove).toHaveBeenCalledWith("a"));
    });
});
