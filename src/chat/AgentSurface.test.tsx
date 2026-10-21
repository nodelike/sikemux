import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import type { Agent, Session } from "../state/types";
import { AgentSurface } from "./AgentSurface";
import { deliverToAgent } from "../agents/agentInbox";
import { registerComposer } from "./draftHandoff";

const mocks = vi.hoisted(() => ({
    chatPane: vi.fn(() => null),
    toggleDesk: vi.fn(),
    renameAgent: vi.fn(),
    typed: vi.fn(),
    terminalProps: vi.fn(),
    screen: [] as string[],
    addAgent: vi.fn(),
    state: { deskPanes: {} as Record<string, string>, windows: {} as Record<string, unknown>, keybindingOverrides: {}, agentStatus: {}, agents: {} },
}));

vi.mock("./AgentChatPane", () => ({ AgentChatPane: mocks.chatPane }));
vi.mock("../terminal/TerminalPane", async () => {
    const { registerTextInsert } = await import("../state/textInsertRegistry");
    const { registerScreen } = await import("../state/terminalScreens");
    return {
        TerminalPane: (props: object) => {
            mocks.terminalProps(props);
            return (
                <div
                    ref={(element) => {
                        if (!element) return;
                        registerTextInsert(element, mocks.typed);
                        registerScreen(element, () => mocks.screen);
                    }}
                />
            );
        },
    };
});
vi.mock("../state/store", () => ({
    useStore: (select: (state: typeof mocks.state) => unknown) => select(mocks.state),
    getState: () => mocks.state,
}));
vi.mock("../state/commands", () => ({
    addAgent: mocks.addAgent,
    toggleDesk: mocks.toggleDesk,
    renameAgent: mocks.renameAgent,
    toggleAgentSkipPermissions: vi.fn(),
}));

const agent: Agent = {
    id: "agent-1",
    type: "claude",
    title: "Agent session",
    permissionMode: "workspace-write",
    launchState: "live",
};

const session: Session = { id: "session-1", name: "repo", kind: "project", cwd: "/repo", pinned: false, activeWindowId: "window-1" };

afterEach(() => {
    cleanup();
    mocks.chatPane.mockClear();
    mocks.toggleDesk.mockClear();
    mocks.renameAgent.mockClear();
    mocks.addAgent.mockClear();
    mocks.state = { deskPanes: {}, windows: {}, keybindingOverrides: {}, agentStatus: {}, agents: {} };
});

/* The window layer keeps a live agent mounted so it keeps its process. The
   adapter and CLI take about a second to come up, so that has to start with the
   pane, not with the first look at it. */
it("connects an agent that is mounted but not on screen", () => {
    render(<AgentSurface agent={agent} session={session} visible={false} />);
    expect(mocks.chatPane).toHaveBeenCalledWith(expect.objectContaining({ active: true, visible: false }), undefined);
});

it("shows the desk toggle as off while the agent's desk is hidden", () => {
    render(<AgentSurface agent={agent} session={session} visible />);
    const toggle = screen.getByRole("button", { name: "Show desk" });
    expect(toggle).toHaveAttribute("aria-pressed", "false");

    fireEvent.click(toggle);
    expect(mocks.toggleDesk).toHaveBeenCalledWith("agent-1");
});

it("shows the desk toggle as on while the agent's desk is in the layout", () => {
    mocks.state = {
        keybindingOverrides: {},
        agentStatus: {},
        agents: {},
        deskPanes: { "desk-1": "agent-1" },
        windows: {
            "window-1": {
                root: {
                    type: "split",
                    id: "split-1",
                    dir: "row",
                    sizes: [0.5, 0.5],
                    children: [
                        { type: "pane", id: "agent-1", cwd: "/repo", kind: "agent", title: "claude" },
                        { type: "pane", id: "desk-1", cwd: "/repo", kind: "desk", title: "desk" },
                    ],
                },
            },
        },
    };

    render(<AgentSurface agent={agent} session={session} visible />);

    expect(screen.getByRole("button", { name: "Hide desk" })).toHaveAttribute("aria-pressed", "true");
});

it("takes what was typed in the chat to the TUI's prompt, without sending it", async () => {
    let draft = "ten lines\nwritten in the chat";
    const stop = registerComposer("agent-1", {
        take: () => {
            const taken = draft;
            draft = "";
            return taken;
        },
        put: () => {},
    });
    render(<AgentSurface agent={agent} session={session} visible />);
    fireEvent.click(screen.getByRole("button", { name: /TUI/ }));

    await vi.waitFor(() =>
        expect(mocks.terminalProps).toHaveBeenLastCalledWith(expect.objectContaining({ initialDraft: "ten lines\nwritten in the chat" })),
    );
    expect(draft).toBe("");
    stop();
});

it("brings what was typed in the TUI's input box back to the chat", async () => {
    let draft = "";
    const stop = registerComposer("agent-1", {
        take: () => "",
        put: (text) => {
            draft = text;
        },
    });
    mocks.screen = ["⏺ Done.", "────────────────", "❯ write the migration", "  and its rollback", "────────────────", "  ⏵⏵ bypass permissions on"];
    render(<AgentSurface agent={agent} session={session} visible />);
    fireEvent.click(screen.getByRole("button", { name: /TUI/ }));
    await vi.waitFor(() => expect(screen.getByRole("button", { name: /GUI/ })).not.toBeDisabled());
    fireEvent.click(screen.getByRole("button", { name: /GUI/ }));

    await vi.waitFor(() => expect(draft).toBe("write the migration\nand its rollback"));
    stop();
    mocks.screen = [];
});

it("keeps Pi in its terminal, since it has no ACP mode", () => {
    render(<AgentSurface agent={{ ...agent, type: "pi" }} session={session} visible />);
    expect(mocks.chatPane).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: /GUI/ })).toBeDisabled();
});

it("renames the agent from its title on double-click", () => {
    render(<AgentSurface agent={agent} session={session} visible />);
    fireEvent.doubleClick(screen.getByText("Agent session"));
    const field = screen.getByRole("textbox", { name: "Title" });
    fireEvent.change(field, { target: { value: "Parser rewrite" } });
    fireEvent.keyDown(field, { key: "Enter" });

    expect(mocks.renameAgent).toHaveBeenCalledWith("agent-1", "Parser rewrite");
    expect(screen.queryByRole("textbox", { name: "Title" })).not.toBeInTheDocument();
});

it("types a delivered issue into a terminal agent as text", () => {
    render(<AgentSurface agent={{ ...agent, type: "pi" }} session={session} visible />);
    deliverToAgent("agent-1", {
        text: "fix this",
        context: [{ uri: "https://github.com/o/r/issues/12", title: "#12 Login crashes", text: "Issue #12: Login crashes" }],
    });
    expect(mocks.typed).toHaveBeenCalledWith(
        "fix this\n\n### #12 Login crashes\nhttps://github.com/o/r/issues/12\n\n```\nIssue #12: Login crashes\n```",
    );
});
