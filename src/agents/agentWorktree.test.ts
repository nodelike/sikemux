import { beforeEach, describe, expect, it, vi } from "vitest";
import { getState, setState } from "../state/store";
import type { Agent, AgentWorktree } from "../state/types";
import { removeAgentWorktree } from "./agentWorktree";

const mocks = vi.hoisted(() => ({
    worktreeRemove: vi.fn(async () => ({})),
    status: vi.fn(),
    compare: vi.fn(),
    branchDelete: vi.fn(async () => {}),
    confirm: vi.fn(async () => true),
    notify: vi.fn(),
    setAgentWorktree: vi.fn(),
}));

vi.mock("../api/git", () => ({
    git: {
        worktreeRemove: mocks.worktreeRemove,
        status: mocks.status,
        compare: mocks.compare,
        branchDelete: mocks.branchDelete,
    },
}));
vi.mock("../state/commands", () => ({ setAgentWorktree: mocks.setAgentWorktree }));
vi.mock("../state/dialog", () => ({ confirmDialog: mocks.confirm }));
vi.mock("../state/toast", async (importOriginal) => ({ ...(await importOriginal<object>()), notify: mocks.notify }));

beforeEach(() => vi.clearAllMocks());

describe("removeAgentWorktree", () => {
    const worktree: AgentWorktree = { repo: "/code/app", path: "/code/app.worktrees/fix", branch: "sikemux/fix", base: "main", startSha: "abc123" };
    const agent: Agent = { id: "agent-1", type: "claude", title: "fix", cwd: worktree.path, worktree };

    beforeEach(() => {
        setState({ agents: { [agent.id]: agent }, sessions: {}, agentStatus: {} });
        mocks.status.mockResolvedValue({ files: [] });
        mocks.compare.mockResolvedValue({ commits: [] });
    });

    it("removes a clean, merged worktree and its branch without asking, and moves the agent home", async () => {
        await removeAgentWorktree(agent.id);
        expect(mocks.confirm).not.toHaveBeenCalled();
        expect(mocks.worktreeRemove).toHaveBeenCalledWith("/code/app", worktree.path, false);
        expect(mocks.setAgentWorktree).toHaveBeenCalledWith(agent.id, "/code/app", null);
        expect(mocks.branchDelete).toHaveBeenCalledWith("/code/app", "sikemux/fix", true);
    });

    it("asks before throwing away uncommitted changes", async () => {
        mocks.status.mockResolvedValue({ files: [{ path: "a.ts", index: " ", worktree: "M" }] });
        mocks.confirm.mockResolvedValueOnce(false);
        await removeAgentWorktree(agent.id);
        expect(mocks.worktreeRemove).not.toHaveBeenCalled();

        await removeAgentWorktree(agent.id);
        expect(mocks.worktreeRemove).toHaveBeenCalledWith("/code/app", worktree.path, true);
    });

    it("asks before deleting a branch with unmerged commits, and keeps it when told to", async () => {
        mocks.compare.mockResolvedValue({ commits: [{ hash: "def" }] });
        mocks.confirm.mockResolvedValueOnce(false);
        await removeAgentWorktree(agent.id);
        expect(mocks.confirm).toHaveBeenCalledWith(expect.objectContaining({ title: "Delete branch sikemux/fix too?" }));
        expect(mocks.worktreeRemove).toHaveBeenCalled();
        expect(mocks.branchDelete).not.toHaveBeenCalled();
    });

    it("trusts a merged pull request over the local history", async () => {
        mocks.compare.mockResolvedValue({ commits: [{ hash: "squashed" }] });
        await removeAgentWorktree(agent.id, true);
        expect(mocks.confirm).not.toHaveBeenCalled();
        expect(mocks.branchDelete).toHaveBeenCalled();
    });

    it("refuses while another agent still works there", async () => {
        setState({ agents: { [agent.id]: agent, other: { ...agent, id: "other", worktree: undefined, cwd: `${worktree.path}/src` } } });
        await removeAgentWorktree(agent.id);
        expect(mocks.worktreeRemove).not.toHaveBeenCalled();
        expect(mocks.notify).toHaveBeenCalledWith("info", expect.stringMatching(/still uses this worktree/));
        expect(getState().agents[agent.id].worktree).toEqual(worktree);
    });
});
