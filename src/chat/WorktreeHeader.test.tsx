import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Pull } from "../codehost/types";
import { resetResourcesForTests } from "../state/resources";
import type { AgentWorktree } from "../state/types";
import WorktreeHeader from "./WorktreeHeader";

const mocks = vi.hoisted(() => ({
    pull: null as Partial<Pull> | null,
    worktrees: vi.fn(async () => [{ path: "/code/app.worktrees/fix", branch: "sikemux/fix", head: "abc" }]),
    compare: vi.fn(async () => ({ commits: [] as unknown[] })),
    openUrlOnDesk: vi.fn(),
    remove: vi.fn(async () => {}),
}));

vi.mock("../codehost/project", async (importOriginal) => ({
    ...(await importOriginal<object>()),
    useHostRepo: () => ({ repo: { provider: "github", owner: "o", name: "app" }, remote: null, branch: null, loading: false }),
    useBranchPull: () => mocks.pull,
}));
vi.mock("../api/git", () => ({ git: { worktrees: mocks.worktrees, compare: mocks.compare } }));
vi.mock("../state/commands", () => ({ openUrlOnDesk: mocks.openUrlOnDesk }));
vi.mock("../agents/agentWorktree", () => ({ removeAgentWorktree: mocks.remove }));

const worktree: AgentWorktree = { repo: "/code/app", path: "/code/app.worktrees/fix", branch: "sikemux/fix", base: "main", startSha: "abc" };
const pull = (fields: Partial<Pull>) => ({
    number: 12,
    title: "Fix the PTY test",
    state: "open",
    draft: false,
    mergedAt: null,
    url: "https://github.com/o/app/pull/12",
    ...fields,
});

beforeEach(() => {
    vi.clearAllMocks();
    resetResourcesForTests();
    mocks.pull = null;
});

afterEach(cleanup);

describe("WorktreeHeader", () => {
    it("shows nothing without a pull request or merged work", async () => {
        const { container } = render(<WorktreeHeader agentId="a" worktree={worktree} visible />);
        await waitFor(() => expect(mocks.compare).toHaveBeenCalledWith("/code/app", "main", "sikemux/fix"));
        expect(container).toBeEmptyDOMElement();
    });

    it("shows the branch's open pull request and opens it beside the agent", () => {
        mocks.pull = pull({});
        render(<WorktreeHeader agentId="a" worktree={worktree} visible />);
        const badge = screen.getByRole("button", { name: /#12/ });
        expect(badge).toHaveTextContent("#12Open");
        fireEvent.click(badge);
        expect(mocks.openUrlOnDesk).toHaveBeenCalledWith("a", "https://github.com/o/app/pull/12");
        expect(screen.queryByRole("button", { name: "Remove worktree" })).not.toBeInTheDocument();
    });

    it("offers to remove the worktree once the pull request is merged", async () => {
        mocks.pull = pull({ state: "closed", mergedAt: "2026-09-30T10:00:00Z" });
        render(<WorktreeHeader agentId="a" worktree={worktree} visible />);
        expect(screen.getByRole("button", { name: /#12/ })).toHaveTextContent("Merged");
        fireEvent.click(screen.getByRole("button", { name: "Remove worktree" }));
        await waitFor(() => expect(mocks.remove).toHaveBeenCalledWith("a", true));
    });

    it("offers it too once the branch is merged into its base locally", async () => {
        mocks.worktrees.mockResolvedValueOnce([{ path: worktree.path, branch: "sikemux/fix", head: "def" }]);
        render(<WorktreeHeader agentId="a" worktree={worktree} visible />);
        fireEvent.click(await screen.findByRole("button", { name: "Remove worktree" }));
        await waitFor(() => expect(mocks.remove).toHaveBeenCalledWith("a", false));
    });
});
