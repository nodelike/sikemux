import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ComponentProps } from "react";
import type { TerminalPane } from "../terminal/TerminalPane";
import * as cmd from "../state/commands";
import { getState, setState } from "../state/store";
import { acpApi } from "../api/acp";
import { Workspace } from "./Workspace";
import { agentIdsOf, agentWindowId } from "../state/selectors";
import { withAgents } from "../test/agents";

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
    const slices = withAgents(state, sessionId, [
        {
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
    ]);
    setState({
        ...slices,
        sessions: {
            ...state.sessions,
            [sessionId]: { ...session, kind: "project", cwd: "/repo", activeWindowId: agentWindowId(slices, "agent-only")! },
        },
    });
    return sessionId;
}

describe("workspace tab bars", () => {
    it("keeps the terminal tab bar and new-tab action visible with one terminal", () => {
        const { container } = render(<Workspace />);

        expect(screen.getByRole("tablist")).toBeInTheDocument();
        expect(screen.getByRole("button", { name: "New tab" })).toBeInTheDocument();
        expect(container.querySelector(".window-area")).toHaveClass("window-area--strip");
    });

    it("keeps the agent tab bar and new-tab action visible with one agent", () => {
        projectWithAgent();

        const { container } = render(<Workspace />);

        expect(screen.getByRole("tablist")).toBeInTheDocument();
        const addTab = screen.getByRole("button", { name: "New tab" });
        expect(addTab).toBeInTheDocument();
        expect(container.querySelector(".window-area")).toHaveClass("window-area--strip");

        fireEvent.click(addTab);
        expect(getState().newTabPaletteOpen).toBe(true);
    });

    it("names the strip the variant its see-through rule is written against", () => {
        projectWithAgent();
        const { container } = render(<Workspace />);

        // base.css strengthens `.tabbar.v-agent` when the window is transparent,
        // where the strip's rule is drawn on the wallpaper with no ground of its
        // own. Renaming the variant would drop that edge without failing a test.
        expect(container.querySelector(".tabbar.v-agent")).toBeInTheDocument();
    });

    it("keeps the agent's tab in the strip once its browser takes focus", () => {
        projectWithAgent();
        render(<Workspace />);
        expect(screen.getByRole("tab", { name: /only agent/ })).toBeInTheDocument();

        act(() => cmd.openBrowserPane("agent-only"));

        // The browser is the focused pane now. Reading the agent off the focused
        // pane finds nothing and drops the tab, stranding the agent.
        expect(screen.getByRole("tab", { name: /only agent/ })).toBeInTheDocument();
    });

    it("updates permission mode from the session composer", async () => {
        projectWithAgent(false);
        render(<Workspace />);

        const toggle = await screen.findByRole("button", { name: /safe/i });
        await waitFor(() => expect(toggle).not.toBeDisabled());
        fireEvent.click(toggle);

        expect(getState().agents["agent-only"]).toMatchObject({
            permissionMode: "bypass",
            skipPermissions: true,
            directCommand: { program: "codex", args: ["--dangerously-bypass-approvals-and-sandbox"] },
        });
        expect(await screen.findByRole("button", { name: /yolo/i })).toHaveAttribute("aria-pressed", "true");
        expect(screen.queryByTestId("terminal-agent-only")).not.toBeInTheDocument();
    });

    it("flips YOLO from the TUI view and relaunches the CLI", async () => {
        projectWithAgent(false);
        render(<Workspace />);

        await act(async () => {
            fireEvent.click(await screen.findByRole("button", { name: "TUI" }));
        });

        const original = await screen.findByTestId("terminal-agent-only");
        const header = document.querySelector(".agent-surface-header") as HTMLElement;
        const toggle = within(header).getByRole("button", { name: /safe/i });
        expect(toggle).toHaveAttribute("aria-pressed", "false");
        await act(async () => {
            fireEvent.click(toggle);
        });

        expect(getState().agents["agent-only"]).toMatchObject({
            permissionMode: "bypass",
            directCommand: { program: "codex", args: ["--dangerously-bypass-approvals-and-sandbox"] },
        });
        expect(toggle).toHaveTextContent("yolo");
        expect(toggle).toHaveAttribute("aria-pressed", "true");
        expect(screen.getByTestId("terminal-agent-only")).not.toBe(original);
    });

    it("keeps the ACP session alive while its tab is hidden", async () => {
        const sessionId = projectWithAgent(false);
        render(<Workspace />);
        await waitFor(() => expect(acpApi.start).toHaveBeenCalledTimes(1));
        await act(async () => {
            setState((state) => ({
                sessions: { ...state.sessions, [sessionId]: { ...state.sessions[sessionId], activeWindowId: state.windowsBySession[sessionId][0] } },
            }));
        });
        expect(acpApi.stop).not.toHaveBeenCalled();
        await act(async () => {
            setState((state) => ({
                sessions: { ...state.sessions, [sessionId]: { ...state.sessions[sessionId], activeWindowId: agentWindowId(state, "agent-only")! } },
            }));
        });
        expect(acpApi.start).toHaveBeenCalledTimes(1);
    });

    it("changes the harness in place before the first message and preserves the draft", async () => {
        const sessionId = projectWithAgent(false);
        render(<Workspace />);
        const editor = await screen.findByRole("textbox", { name: "Message agent" });
        await waitFor(() => expect(editor).toBeEnabled());
        fireEvent.change(editor, { target: { value: "Keep this draft" } });
        fireEvent.click(screen.getByRole("button", { name: "Model" }));
        fireEvent.click(within(screen.getByRole("group", { name: "Agent" })).getByRole("button", { name: /Claude/ }));
        await waitFor(() => expect(acpApi.start).toHaveBeenCalledTimes(2));
        expect(acpApi.start).toHaveBeenLastCalledWith(expect.objectContaining({ agentId: "agent-only", provider: "claude", resumeId: undefined }));
        expect(acpApi.stop).toHaveBeenCalledWith("agent-only");
        expect(agentIdsOf(getState(), sessionId)).toEqual(["agent-only"]);
        expect(getState().agents["agent-only"]).toMatchObject({ type: "claude", title: "Claude" });
        expect(editor).toHaveValue("Keep this draft");
        expect(within(screen.getByRole("group", { name: "Agent" })).getByRole("button", { name: /Claude/ })).toHaveAttribute("aria-pressed", "true");
        expect(screen.queryByText("only agent")).not.toBeInTheDocument();
        expect(screen.getAllByText("Claude").length).toBeGreaterThan(0);
    });

    it("shows YOLO inside the session composer", async () => {
        projectWithAgent();
        setState((state) => ({
            agents: { ...state.agents, "agent-only": { ...state.agents["agent-only"], permissionMode: "bypass", skipPermissions: true } },
        }));

        render(<Workspace />);

        expect(await screen.findByRole("button", { name: /yolo/i })).toHaveAttribute("aria-pressed", "true");
    });

    it("puts windows and agents in one tab strip", () => {
        projectWithAgent();

        render(<Workspace />);

        const tabs = screen.getAllByRole("tab").map((tab) => tab.textContent);
        expect(tabs).toContain("only agent");
        expect(tabs.length).toBeGreaterThan(1);
    });

    it("switches from an agent tab to a window tab through the same strip", () => {
        projectWithAgent();
        render(<Workspace />);

        const sessionId = getState().activeSessionId;
        const windowId = getState().windowsBySession[sessionId][0];
        const windowTab = screen.getAllByRole("tab").find((tab) => tab.getAttribute("aria-selected") === "false");
        expect(windowTab).toBeDefined();

        fireEvent.click(windowTab!);

        expect(getState().sessions[sessionId].activeWindowId).toBe(windowId);
    });
});

describe("stage layers", () => {
    function addAgentTo(sessionId: string) {
        setState((s) => withAgents(s, sessionId, [{ id: "agent-two", type: "codex", title: "second agent", startup: "codex", launchState: "live" }]));
    }

    /*
     * A document tab lighting no layer, or two layers at once, both read as the
     * editor and an agent painting over each other on one stage.
     */
    it("shows exactly one layer when a document tab is active alongside an agent", () => {
        const sessionId = getState().activeSessionId;
        addAgentTo(sessionId);
        cmd.requestOpenFile("/repo/a.ts");

        const { container } = render(<Workspace />);

        expect(container.querySelectorAll(".window-layer.live")).toHaveLength(1);
    });

    it("still renders the editor layer when it holds no document", () => {
        cmd.openEditorPane();

        const { container } = render(<Workspace />);

        expect(container.querySelectorAll(".window-layer.live")).toHaveLength(1);
    });
});
