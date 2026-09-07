import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ComponentProps } from "react";
import type { TerminalPane } from "../terminal/TerminalPane";
import { getState, setState } from "../state/store";
import { acpApi } from "../api/acp";
import { Workspace } from "./Workspace";

vi.mock("../api/acp", () => ({
    acpApi: {
        subscribe: vi.fn(async () => () => {}),
        start: vi.fn(async () => ({ sessionId: "session-only", capabilities: {}, setup: {} })),
        setPermissionMode: vi.fn(async () => {}),
        stop: vi.fn(async () => {}),
        prompt: vi.fn(async () => {}),
        cancel: vi.fn(async () => {}),
        permissionReply: vi.fn(async () => {}),
    },
}));

vi.mock("../terminal/TerminalPane", () => ({
    TerminalPane: (props: ComponentProps<typeof TerminalPane>) => (
        <div data-testid={`terminal-${props.context?.agentId ?? "window"}`} data-visible={String(props.visible)} />
    ),
}));

const initial = getState();

beforeEach(() => {
    vi.clearAllMocks();
    setState(initial, true);
});
afterEach(cleanup);

function projectWithAgent(resumable = true): string {
    const state = getState();
    const sessionId = state.activeSessionId;
    const session = state.sessions[sessionId];
    setState({
        sessions: {
            ...state.sessions,
            [sessionId]: { ...session, kind: "project", view: "agent", activeAgentId: "agent-only", cwd: "/repo" },
        },
        agents: {
            "agent-only": {
                id: "agent-only",
                type: "codex",
                title: "only agent",
                startup: "codex",
                directCommand: {
                    program: "codex",
                    args: resumable ? ["resume", "--sandbox", "workspace-write", "session-only"] : ["--sandbox", "workspace-write"],
                },
                ...(resumable ? { resumeId: "session-only" } : {}),
                permissionMode: "workspace-write",
                launchState: "live",
            },
        },
        agentsBySession: { ...state.agentsBySession, [sessionId]: ["agent-only"] },
    });
    return sessionId;
}

describe("workspace tab bars", () => {
    it("keeps the terminal tab bar and new-terminal action visible with one terminal", () => {
        const { container } = render(<Workspace />);

        expect(screen.getByRole("tablist")).toBeInTheDocument();
        expect(screen.getByRole("button", { name: "New terminal — ⌥N" })).toBeInTheDocument();
        expect(container.querySelector(".window-layer.visible")).toHaveStyle({ top: "34px" });
    });

    it("keeps the agent tab bar and new-agent action visible with one agent", () => {
        projectWithAgent();

        const { container } = render(<Workspace />);

        expect(screen.getByRole("tablist")).toBeInTheDocument();
        const addAgent = screen.getByRole("button", { name: "New agent — ⌥N" });
        expect(addAgent).toBeInTheDocument();
        expect(container.querySelector(".window-layer.visible .pane-cell")).toHaveStyle({ top: "34px", height: "calc(100% - 34px)" });

        fireEvent.click(addAgent);
        expect(getState().agentPaletteOpen).toBe(true);
    });

    it("updates permission mode from the session composer", async () => {
        projectWithAgent(false);
        render(<Workspace />);

        const toggle = await screen.findByRole("button", { name: /normal/i });
        await waitFor(() => expect(toggle).not.toBeDisabled());
        fireEvent.click(toggle);

        expect(getState().agents["agent-only"]).toMatchObject({
            permissionMode: "bypass",
            skipPermissions: true,
            directCommand: { program: "codex", args: ["--dangerously-bypass-approvals-and-sandbox"] },
        });
        expect(await screen.findByRole("button", { name: /yolo/i })).toHaveClass("chat-permission-mode", "tone-danger");
        expect(screen.queryByTestId("terminal-agent-only")).not.toBeInTheDocument();
    });

    it("keeps the ACP session alive while its tab is hidden", async () => {
        const sessionId = projectWithAgent(false);
        render(<Workspace />);
        await waitFor(() => expect(acpApi.start).toHaveBeenCalledTimes(1));
        await act(async () => {
            setState((state) => ({ sessions: { ...state.sessions, [sessionId]: { ...state.sessions[sessionId], view: "windows" } } }));
        });
        expect(acpApi.stop).not.toHaveBeenCalled();
        await act(async () => {
            setState((state) => ({ sessions: { ...state.sessions, [sessionId]: { ...state.sessions[sessionId], view: "agent" } } }));
        });
        expect(acpApi.start).toHaveBeenCalledTimes(1);
    });

    it("changes the harness in place before the first message and preserves the draft", async () => {
        const sessionId = projectWithAgent(false);
        render(<Workspace />);
        const editor = await screen.findByRole("textbox", { name: "Message agent" });
        await waitFor(() => expect(editor).toBeEnabled());
        fireEvent.change(editor, { target: { value: "Keep this draft" } });
        fireEvent.click(screen.getByRole("button", { name: "Agent" }));
        fireEvent.click(screen.getByRole("option", { name: /^Claude\s*Default configuration$/ }));
        await waitFor(() => expect(acpApi.start).toHaveBeenCalledTimes(2));
        expect(acpApi.start).toHaveBeenLastCalledWith(expect.objectContaining({ agentId: "agent-only", provider: "claude", resumeId: undefined }));
        expect(acpApi.stop).toHaveBeenCalledWith("agent-only");
        expect(getState().agentsBySession[sessionId]).toEqual(["agent-only"]);
        expect(getState().agents["agent-only"]).toMatchObject({ type: "claude", title: "claude" });
        expect(editor).toHaveValue("Keep this draft");
        expect(screen.getByRole("button", { name: "Agent" })).toHaveTextContent("Claude");
        expect(screen.queryByText("only agent")).not.toBeInTheDocument();
        expect(screen.getAllByText("claude").length).toBeGreaterThan(0);
    });

    it("shows YOLO inside the session composer", async () => {
        projectWithAgent();
        setState((state) => ({
            agents: { ...state.agents, "agent-only": { ...state.agents["agent-only"], permissionMode: "bypass", skipPermissions: true } },
        }));

        render(<Workspace />);

        expect(await screen.findByRole("button", { name: /yolo/i })).toHaveClass("chat-permission-mode", "tone-danger");
    });

    it("requests the agent picker for the empty agent stage", () => {
        const state = getState();
        const sessionId = state.activeSessionId;
        setState({
            sessions: {
                ...state.sessions,
                [sessionId]: { ...state.sessions[sessionId], kind: "project", view: "agent", activeAgentId: null, cwd: "/repo" },
            },
            agentsBySession: { ...state.agentsBySession, [sessionId]: [] },
        });

        render(<Workspace />);

        expect(screen.getByText("no agents in this project")).toBeInTheDocument();
        expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
        expect(getState().agentPaletteOpen).toBe(true);
    });
});
