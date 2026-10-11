import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PreparedWorktree } from "../agents/agentWorktree";
import { resetResourcesForTests } from "../state/resources";
import { setState } from "../state/store";
import type { Agent, AgentWorktree } from "../state/types";
import { AgentChatPane } from "./AgentChatPane";

const mocks = vi.hoisted(() => ({
    prompt: vi.fn(async () => {}),
    start: vi.fn(async () => ({ sessionId: "session-1", capabilities: {}, setup: {} })),
    create: vi.fn(),
    setAgentWorktree: vi.fn(),
    setAgentWorktreeDefault: vi.fn(),
    worktrees: vi.fn(async () => [{ path: "/repo", head: "abc", branch: "main", is_main: true, current: true }]),
}));

vi.mock("../api/agents", () => ({ agentApi: { sessionContext: async () => null, available: async () => [] } }));
vi.mock("../api/fs", () => ({ fsapi: { pathKinds: async (paths: string[]) => paths.map(() => null) } }));
vi.mock("../api/git", () => ({ git: { worktrees: mocks.worktrees, overview: async () => ({ status: { branch: "main" } }) } }));
vi.mock("../agents/agentWorktree", () => ({ createAgentWorktree: mocks.create }));
vi.mock("../api/acp", () => ({
    acpApi: {
        subscribe: vi.fn(async () => () => {}),
        attach: vi.fn(async () => ({ status: "missing" })),
        start: mocks.start,
        setPermissionMode: vi.fn(async () => {}),
        setConfig: vi.fn(),
        stop: vi.fn(async () => {}),
        prompt: mocks.prompt,
        steer: vi.fn(),
        cancel: vi.fn(async () => {}),
        stopTask: vi.fn(),
        permissionReply: vi.fn(),
    },
}));
vi.mock("../state/commands", () => ({
    setAgentWorktree: mocks.setAgentWorktree,
    setAgentWorktreeDefault: mocks.setAgentWorktreeDefault,
    attachAgentSession: vi.fn(),
    setAgentPermissionMode: vi.fn(),
    setAgentModelPreferences: vi.fn(),
    setAgentTitle: vi.fn(),
    titleAgentFromPrompt: vi.fn(),
    noteAcpAgentState: vi.fn(),
    noteAgentBackgroundWork: vi.fn(),
    toggleAgentSkipPermissions: vi.fn(),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => null) }));

const agent: Agent = {
    id: "agent-wt",
    type: "codex",
    title: "Agent",
    startup: "codex",
    permissionMode: "workspace-write",
    launchState: "live",
    cwd: "/repo",
};
const worktree: AgentWorktree = {
    repo: "/repo",
    path: "/repo.worktrees/fix-the-flaky-pty-test",
    branch: "sikemux/fix-the-flaky-pty-test",
    base: "main",
    startSha: "abc",
};

let resolveCreate: (prepared: PreparedWorktree) => void = () => {};

function renderPane() {
    const view = render(<AgentChatPane agent={agent} cwd="/repo" active visible onBusyChange={() => {}} />);
    mocks.setAgentWorktree.mockImplementation((_id: string, cwd: string, moved: AgentWorktree | null) => {
        view.rerender(
            <AgentChatPane agent={{ ...agent, cwd, ...(moved ? { worktree: moved } : {}) }} cwd={cwd} active visible onBusyChange={() => {}} />,
        );
    });
    return view;
}

async function sendFirstMessage(text: string): Promise<HTMLTextAreaElement> {
    const editor = screen.getByRole("textbox", { name: "Message agent" }) as HTMLTextAreaElement;
    await waitFor(() => expect(mocks.start).toHaveBeenCalledWith(expect.objectContaining({ cwd: "/repo" })));
    const toggle = await screen.findByRole("button", { name: "worktree" });
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-pressed", "true");
    fireEvent.change(editor, { target: { value: text } });
    fireEvent.keyDown(editor, { key: "Enter" });
    return editor;
}

beforeEach(() => {
    vi.clearAllMocks();
    resetResourcesForTests();
    setState({ agentWorktreeDefaults: {} });
    mocks.create.mockImplementation(
        () =>
            new Promise<PreparedWorktree>((resolve) => {
                resolveCreate = resolve;
            }),
    );
});

afterEach(cleanup);

describe("AgentChatPane in a worktree", () => {
    it("moves a fresh chat into a new worktree before its first message goes out", async () => {
        renderPane();
        await sendFirstMessage("Fix the flaky PTY test");

        await waitFor(() =>
            expect(mocks.create).toHaveBeenCalledWith(
                expect.objectContaining({ agentId: agent.id, projectCwd: "/repo", message: "Fix the flaky PTY test" }),
            ),
        );
        expect(mocks.setAgentWorktreeDefault).toHaveBeenCalledWith("/repo", true);
        expect(screen.getByRole("button", { name: "Send message" })).toBeDisabled();
        expect(mocks.prompt).not.toHaveBeenCalled();

        resolveCreate({ worktree, cwd: worktree.path, setupError: null });
        await waitFor(() => expect(mocks.prompt).toHaveBeenCalledWith(agent.id, "Fix the flaky PTY test", [], [], expect.any(String)));
        expect(mocks.start).toHaveBeenLastCalledWith(expect.objectContaining({ cwd: worktree.path }));
        expect(mocks.prompt).toHaveBeenCalledTimes(1);
        expect(await screen.findByRole("note")).toHaveTextContent("Working in worktree sikemux/fix-the-flaky-pty-test");
        expect(await screen.findByRole("button", { name: "sikemux/fix-the-flaky-pty-test" })).toBeDisabled();
    });

    it("keeps the worktree and gives the message back when its setup fails", async () => {
        renderPane();
        const editor = await sendFirstMessage("Fix the flaky PTY test");
        await waitFor(() => expect(mocks.create).toHaveBeenCalled());

        resolveCreate({ worktree, cwd: worktree.path, setupError: "Install failed: exit 1" });
        expect(await screen.findByText(/Install failed: exit 1\. The worktree is kept at/)).toBeInTheDocument();
        await waitFor(() => expect(editor.value).toBe("Fix the flaky PTY test"));
        expect(mocks.setAgentWorktree).toHaveBeenCalledWith(agent.id, worktree.path, worktree);
        expect(mocks.prompt).not.toHaveBeenCalled();
    });

    it("leaves the chat where it is when the worktree cannot be made", async () => {
        mocks.create.mockRejectedValueOnce(new Error("branch exists"));
        renderPane();
        const editor = await sendFirstMessage("Fix the flaky PTY test");

        expect(await screen.findByText("Could not create a worktree: branch exists")).toBeInTheDocument();
        await waitFor(() => expect(editor.value).toBe("Fix the flaky PTY test"));
        expect(mocks.setAgentWorktree).not.toHaveBeenCalled();
        expect(mocks.prompt).not.toHaveBeenCalled();
    });

    it("sends straight away with the switch off", async () => {
        renderPane();
        const editor = screen.getByRole("textbox", { name: "Message agent" });
        await screen.findByRole("button", { name: "worktree" });
        await waitFor(() => expect(screen.getByRole("button", { name: "worktree" })).toBeEnabled());
        fireEvent.change(editor, { target: { value: "hello" } });
        fireEvent.keyDown(editor, { key: "Enter" });
        await waitFor(() => expect(mocks.prompt).toHaveBeenCalledWith(agent.id, "hello", [], [], expect.any(String)));
        expect(mocks.create).not.toHaveBeenCalled();
    });
});
