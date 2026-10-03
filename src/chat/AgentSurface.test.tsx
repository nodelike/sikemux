import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import type { Agent, Session } from "../state/types";
import { AgentSurface } from "./AgentSurface";
import { deliverToAgent } from "../agents/agentInbox";

const mocks = vi.hoisted(() => ({
    chatPane: vi.fn(() => null),
    toggleDesk: vi.fn(),
    openDeskSimulator: vi.fn(),
    simulatorsAvailable: false,
    renameAgent: vi.fn(),
    typed: vi.fn(),
    addAgent: vi.fn(),
    relaunch: vi.fn(async () => {}),
    clearRecovery: vi.fn(),
    resume: { recovery: null as null | { phase: "resuming" } | { phase: "failed"; detail: string | null }, generation: 0 },
    state: {
        deskPanes: {} as Record<string, string>,
        desks: {},
        iosSimulator: true,
        windows: {} as Record<string, unknown>,
        keybindingOverrides: {},
    },
}));

vi.mock("./AgentChatPane", () => ({ AgentChatPane: mocks.chatPane }));
vi.mock("../terminal/TerminalPane", async () => {
    const { registerTextInsert } = await import("../state/textInsertRegistry");
    return { TerminalPane: () => <div ref={(element) => (element ? registerTextInsert(element, mocks.typed) : undefined)} /> };
});
vi.mock("../state/store", () => ({
    useStore: (select: (state: typeof mocks.state) => unknown) => select(mocks.state),
    getState: () => mocks.state,
}));
vi.mock("../agents/tuiResume", () => ({
    useTuiResume: () => mocks.resume,
    relaunchTuiAgent: mocks.relaunch,
    clearTuiRecovery: mocks.clearRecovery,
}));
vi.mock("../state/simulatorAvailable", () => ({ useSimulatorsAvailable: () => mocks.simulatorsAvailable }));
vi.mock("../api/simulator", () => ({
    simulatorApi: {
        preferred: vi.fn(async () => ({ udid: "U1", name: "iPhone 17", os: "iOS 27.0", booted: false, screen: { width: 402, height: 874 } })),
    },
}));
vi.mock("../state/commands", () => ({
    addAgent: mocks.addAgent,
    toggleDesk: mocks.toggleDesk,
    openDeskSimulator: mocks.openDeskSimulator,
    renameAgent: mocks.renameAgent,
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
    mocks.renameAgent.mockClear();
    mocks.addAgent.mockClear();
    mocks.relaunch.mockClear();
    mocks.resume = { recovery: null, generation: 0 };
    mocks.state = { deskPanes: {}, desks: {}, iosSimulator: true, windows: {}, keybindingOverrides: {} };
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
        desks: {},
        iosSimulator: true,
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

it("keeps Pi in its terminal, since it has no ACP mode", () => {
    render(<AgentSurface agent={{ ...agent, type: "pi" }} session={session} visible />);
    expect(mocks.chatPane).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: /GUI/ })).toBeDisabled();
});

it("renames the agent from its title on double-click", () => {
    render(<AgentSurface agent={agent} session={session} visible />);
    fireEvent.doubleClick(screen.getByText("Agent session"));
    const field = screen.getByRole("textbox", { name: "Chat name" });
    fireEvent.change(field, { target: { value: "Parser rewrite" } });
    fireEvent.keyDown(field, { key: "Enter" });

    expect(mocks.renameAgent).toHaveBeenCalledWith("agent-1", "Parser rewrite");
    expect(screen.queryByRole("textbox", { name: "Chat name" })).not.toBeInTheDocument();
});

it("types a delivered issue into a terminal agent as text", () => {
    render(<AgentSurface agent={{ ...agent, type: "pi", startup: "pi" }} session={session} visible />);
    deliverToAgent("agent-1", {
        text: "fix this",
        context: [{ uri: "https://github.com/o/r/issues/12", title: "#12 Login crashes", text: "Issue #12: Login crashes" }],
    });
    expect(mocks.typed).toHaveBeenCalledWith(
        "fix this\n\n### #12 Login crashes\nhttps://github.com/o/r/issues/12\n\n```\nIssue #12: Login crashes\n```",
    );
});

it("says quietly that a terminal agent is resuming", () => {
    mocks.resume = { recovery: { phase: "resuming" }, generation: 0 };
    render(<AgentSurface agent={{ ...agent, type: "pi", startup: "pi" }} session={session} visible />);
    expect(screen.getByRole("status")).toHaveTextContent("Resuming…");
    expect(screen.queryByText("Couldn't resume this agent")).not.toBeInTheDocument();
});

it("offers Retry and Start new chat when a terminal agent could not be resumed", () => {
    mocks.resume = { recovery: { phase: "failed", detail: "The agent exited with code 1." }, generation: 0 };
    const crashed: Agent = { ...agent, type: "pi", startup: "pi", resumeId: "r1", ptyId: 40, profileId: "pi-work", cwd: "/repo/sub" };
    render(<AgentSurface agent={crashed} session={session} visible />);
    expect(screen.getByText("Couldn't resume this agent")).toBeInTheDocument();
    expect(screen.getByText("The agent exited with code 1.")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(mocks.relaunch).toHaveBeenCalledWith("agent-1", 40);

    fireEvent.click(screen.getByRole("button", { name: "Start new chat" }));
    expect(mocks.addAgent).toHaveBeenCalledWith("pi", undefined, undefined, {
        permissionMode: "workspace-write",
        profileId: "pi-work",
        detectedExecutablePath: undefined,
        cwd: "/repo/sub",
    });
});

it("shows no resume state in the chat view", () => {
    mocks.resume = { recovery: { phase: "failed", detail: null }, generation: 0 };
    render(<AgentSurface agent={agent} session={session} visible />);
    expect(screen.queryByText("Couldn't resume this agent")).not.toBeInTheDocument();
    expect(screen.queryByText("Resuming…")).not.toBeInTheDocument();
});

it("offers the iOS Simulator only on a Mac that can run it", () => {
    mocks.simulatorsAvailable = false;
    render(<AgentSurface agent={agent} session={session} visible />);
    expect(screen.queryByRole("button", { name: "iOS Simulator" })).toBeNull();
});

it("opens the agent's desk on the simulator it would use", async () => {
    mocks.simulatorsAvailable = true;
    render(<AgentSurface agent={agent} session={session} visible />);

    fireEvent.click(screen.getByRole("button", { name: "iOS Simulator" }));

    await vi.waitFor(() =>
        expect(mocks.openDeskSimulator).toHaveBeenCalledWith("agent-1", {
            udid: "U1",
            name: "iPhone 17",
            os: "iOS 27.0",
            screen: { width: 402, height: 874 },
        }),
    );
    mocks.simulatorsAvailable = false;
});

it("leaves the iOS Simulator out once the person turns it off in Settings", () => {
    mocks.simulatorsAvailable = true;
    mocks.state = { ...mocks.state, iosSimulator: false };
    render(<AgentSurface agent={agent} session={session} visible />);
    expect(screen.queryByRole("button", { name: "iOS Simulator" })).toBeNull();
    mocks.simulatorsAvailable = false;
});

it("leaves the iOS Simulator out of an SSH session, whose agent works on another machine", () => {
    mocks.simulatorsAvailable = true;
    render(<AgentSurface agent={agent} session={{ ...session, kind: "ssh" }} visible />);
    expect(screen.queryByRole("button", { name: "iOS Simulator" })).toBeNull();
    mocks.simulatorsAvailable = false;
});
