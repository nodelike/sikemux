import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import type { Agent, Session } from "../state/types";
import { AgentSurface } from "./AgentSurface";

const mocks = vi.hoisted(() => ({
    chatPane: vi.fn(() => null),
    toggleBrowserPane: vi.fn(),
    state: { browserPanes: {} as Record<string, string>, windows: {} as Record<string, unknown> },
}));

vi.mock("./AgentChatPane", () => ({ AgentChatPane: mocks.chatPane }));
vi.mock("../terminal/TerminalPane", () => ({ TerminalPane: () => null }));
vi.mock("../state/store", () => ({ useStore: (select: (state: typeof mocks.state) => unknown) => select(mocks.state) }));
vi.mock("../state/commands", () => ({
    toggleBrowserPane: mocks.toggleBrowserPane,
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
    mocks.toggleBrowserPane.mockClear();
    mocks.state = { browserPanes: {}, windows: {} };
});

/* The window layer keeps a live agent mounted so it keeps its process. The
   adapter and CLI take about a second to come up, so that has to start with the
   pane, not with the first look at it. */
it("connects an agent that is mounted but not on screen", () => {
    render(<AgentSurface agent={agent} session={session} visible={false} />);
    expect(mocks.chatPane).toHaveBeenCalledWith(expect.objectContaining({ active: true, visible: false }), undefined);
});

it("shows the browser toggle as off while the agent's browser is hidden", () => {
    render(<AgentSurface agent={agent} session={session} visible />);
    const toggle = screen.getByRole("button", { name: "Show browser" });
    expect(toggle).toHaveAttribute("aria-pressed", "false");

    fireEvent.click(toggle);
    expect(mocks.toggleBrowserPane).toHaveBeenCalledWith("agent-1");
});

it("shows the browser toggle as on while the agent's browser is in the layout", () => {
    mocks.state = {
        browserPanes: { "browser-1": "agent-1" },
        windows: {
            "window-1": {
                root: {
                    type: "split",
                    id: "split-1",
                    dir: "row",
                    sizes: [0.5, 0.5],
                    children: [
                        { type: "pane", id: "agent-1", cwd: "/repo", kind: "agent", title: "claude" },
                        { type: "pane", id: "browser-1", cwd: "/repo", kind: "browser", title: "browser" },
                    ],
                },
            },
        },
    };

    render(<AgentSurface agent={agent} session={session} visible />);

    expect(screen.getByRole("button", { name: "Hide browser" })).toHaveAttribute("aria-pressed", "true");
});
