import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ProjectConfigLoadResult } from "../projects/projectConfig";
import { getState, setState } from "../state/store";
import type { Agent, AgentWorktree } from "../state/types";
import { createAgentWorktree, removeAgentWorktree } from "./agentWorktree";

const mocks = vi.hoisted(() => ({
    worktrees: vi.fn(),
    branches: vi.fn(),
    worktreeCreate: vi.fn(),
    worktreeRemove: vi.fn(async () => ({})),
    status: vi.fn(),
    compare: vi.fn(),
    branchDelete: vi.fn(async () => {}),
    pathKinds: vi.fn(async (paths: string[]) => paths.map(() => null as string | null)),
    loadConfig: vi.fn(async (): Promise<ProjectConfigLoadResult> => ({ status: "absent", path: "/code/app/sikemux.json" })),
    trust: vi.fn(async () => true),
    runBackground: vi.fn(async () => ({ code: 0, output: "" })),
    confirm: vi.fn(async () => true),
    notify: vi.fn(),
    setAgentWorktree: vi.fn(),
}));

vi.mock("../api/git", () => ({
    git: {
        worktrees: mocks.worktrees,
        branches: mocks.branches,
        worktreeCreate: mocks.worktreeCreate,
        worktreeRemove: mocks.worktreeRemove,
        status: mocks.status,
        compare: mocks.compare,
        branchDelete: mocks.branchDelete,
    },
}));
vi.mock("../api/fs", () => ({ fsapi: { pathKinds: mocks.pathKinds } }));
vi.mock("../projects/projectConfig", () => ({ loadProjectConfig: mocks.loadConfig }));
vi.mock("../projects/projectConfigRuntime", () => ({
    trustProjectConfig: mocks.trust,
    worktreeHookCommand: (hook: { id: string; label: string; command: string }) => ({ id: hook.id, title: hook.label, command: hook.command }),
}));
vi.mock("../projects/controllerBridge", () => ({ projectControllerBridge: { refresh: vi.fn(async () => {}) } }));
vi.mock("../state/commands", () => ({ setAgentWorktree: mocks.setAgentWorktree }));
vi.mock("../state/commands/customCommands", () => ({ runBackgroundCommand: mocks.runBackground }));
vi.mock("../state/dialog", () => ({ confirmDialog: mocks.confirm }));
vi.mock("../state/toast", async (importOriginal) => ({ ...(await importOriginal<object>()), notify: mocks.notify }));

const main = { path: "/code/app", head: "abc123", branch: "main", is_main: true, current: true };

const validConfig = (hooks: { id: string; label: string; command: string }[]): ProjectConfigLoadResult =>
    ({
        status: "valid",
        path: "/code/app/sikemux.json",
        fingerprint: "f",
        config: { version: 1, actions: [], tasks: [], worktree: { onCreate: hooks } },
        trust: { requiresApproval: true },
    }) as unknown as ProjectConfigLoadResult;

beforeEach(() => {
    vi.clearAllMocks();
    mocks.worktrees.mockResolvedValue([main]);
    mocks.branches.mockResolvedValue([{ name: "main", current: true, upstream: null }]);
    mocks.worktreeCreate.mockImplementation(async (_repo: string, options: { path: string; branch: string }) => ({
        path: options.path,
        branch: options.branch,
    }));
});

describe("createAgentWorktree", () => {
    it("branches off HEAD into a folder beside the repository, skipping names already taken", async () => {
        mocks.branches.mockResolvedValue([{ name: "sikemux/fix-flaky-pty-test", current: false, upstream: null }]);
        mocks.pathKinds.mockImplementationOnce(async (paths: string[]) => paths.map((path) => (path.endsWith("-2") ? "dir" : null)));
        const steps: string[] = [];

        const prepared = await createAgentWorktree({
            agentId: "a",
            projectCwd: "/code/app",
            message: "Fix flaky PTY test",
            onStep: (step) => steps.push(step),
        });

        expect(mocks.worktreeCreate).toHaveBeenCalledWith("/code/app", {
            path: "/code/app.worktrees/fix-flaky-pty-test-3",
            branch: "sikemux/fix-flaky-pty-test-3",
            createBranch: true,
            startPoint: "abc123",
        });
        expect(prepared).toEqual({
            worktree: {
                repo: "/code/app",
                path: "/code/app.worktrees/fix-flaky-pty-test-3",
                branch: "sikemux/fix-flaky-pty-test-3",
                base: "main",
                startSha: "abc123",
            },
            cwd: "/code/app.worktrees/fix-flaky-pty-test-3",
            setupError: null,
        });
        expect(steps).toEqual(["Creating worktree"]);
    });

    it("opens the same subfolder the project had open", async () => {
        const prepared = await createAgentWorktree({ agentId: "a", projectCwd: "/code/app/packages/web", message: "tidy", onStep: () => {} });
        expect(prepared.cwd).toBe("/code/app.worktrees/tidy/packages/web");
    });

    it("runs the project's worktree setup in the new checkout once it is trusted, and stops at the first failure", async () => {
        mocks.loadConfig.mockResolvedValueOnce(
            validConfig([
                { id: "deps", label: "Install", command: "pnpm install" },
                { id: "env", label: "Copy env", command: "cp ../app/.env ." },
                { id: "never", label: "Never", command: "true" },
            ]),
        );
        mocks.runBackground.mockResolvedValueOnce({ code: 0, output: "" }).mockRejectedValueOnce(new Error("Copy env failed: exit 1"));
        const steps: string[] = [];

        const prepared = await createAgentWorktree({ agentId: "a", projectCwd: "/code/app", message: "tidy", onStep: (step) => steps.push(step) });

        expect(mocks.trust).toHaveBeenCalledTimes(1);
        expect(mocks.runBackground).toHaveBeenCalledTimes(2);
        expect(mocks.runBackground).toHaveBeenNthCalledWith(
            1,
            expect.objectContaining({ command: "pnpm install" }),
            "/code/app.worktrees/tidy",
            true,
            undefined,
        );
        expect(prepared.setupError).toBe("Copy env failed: exit 1");
        expect(steps).toEqual(["Creating worktree", "Waiting for sikemux.json to be trusted", "Running Install", "Running Copy env"]);
    });

    it("runs nothing the person did not trust", async () => {
        mocks.loadConfig.mockResolvedValueOnce(validConfig([{ id: "deps", label: "Install", command: "pnpm install" }]));
        mocks.trust.mockResolvedValueOnce(false);
        const prepared = await createAgentWorktree({ agentId: "a", projectCwd: "/code/app", message: "tidy", onStep: () => {} });
        expect(mocks.runBackground).not.toHaveBeenCalled();
        expect(prepared.setupError).toMatch(/not trusted/);
    });
});

describe("removeAgentWorktree", () => {
    const worktree: AgentWorktree = { repo: "/code/app", path: "/code/app.worktrees/fix", branch: "sikemux/fix", base: "main", startSha: "abc123" };
    const agent: Agent = { id: "agent-1", type: "claude", title: "fix", startup: "claude", cwd: worktree.path, worktree };

    beforeEach(() => {
        setState({ agents: { [agent.id]: agent }, sessions: {}, agentActivity: {} });
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
