import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import type { Agent, Session } from "../state/types";
import { AgentSurface } from "./AgentSurface";

const mocks = vi.hoisted(() => ({
    chatPane: vi.fn(() => null),
    toggleDesk: vi.fn(),
    state: { deskPanes: {} as Record<string, string>, windows: {} as Record<string, unknown> },
}));

vi.mock("./AgentChatPane", () => ({ AgentChatPane: mocks.chatPane }));
vi.mock("../terminal/TerminalPane", () => ({ TerminalPane: () => null }));
vi.mock("../state/store", () => ({ useStore: (select: (state: typeof mocks.state) => unknown) => select(mocks.state) }));
vi.mock("../state/commands", () => ({
    toggleDesk: mocks.toggleDesk,
    toggleAgentSkipPermissions: vi.fn(),
    agentSupportsSkipPermissions: () => true,
}));

const agent: Agent = {
    id: "agent-1",
    type: "claude",
    title: "Agent session",
    startup: "claude",
    permissionMode: "workspace-write",
    launchState: "live",
};

const session: Session = { id: "session-1", name: "repo", kind: "project", cwd: "/repo", pinned: false, activeWindowId: "window-1" };

afterEach(() => {
    cleanup();
    mocks.chatPane.mockClear();
    mocks.toggleDesk.mockClear();
    mocks.state = { deskPanes: {}, windows: {} };
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

it("opens agents that speak ACP themselves in the chat", () => {
    for (const type of ["opencode", "omp", "grok", "hermes"] as const) {
        mocks.chatPane.mockClear();
        render(<AgentSurface agent={{ ...agent, type }} session={session} visible />);
        expect(mocks.chatPane).toHaveBeenCalled();
        cleanup();
    }
});

it("keeps Pi in its terminal, since it has no ACP mode", () => {
    render(<AgentSurface agent={{ ...agent, type: "pi" }} session={session} visible />);
    expect(mocks.chatPane).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: /GUI/ })).toBeDisabled();
});
