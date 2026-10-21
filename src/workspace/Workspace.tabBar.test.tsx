import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ComponentProps } from "react";
import type { TerminalPane } from "../terminal/TerminalPane";
import * as cmd from "../state/commands";
import { getState, setState } from "../state/store";
import { acpApi } from "../api/acp";
import { Workspace, WorkspaceTabs } from "./Workspace";
import { agentIdsOf, agentWindowId } from "../state/selectors";
import { withAgents } from "../test/agents";
import { acceptDialog, useDialogs } from "../state/dialog";
import { draftOf, typeDraft } from "../test/draft";

vi.mock("../api/acp", () => {
    /* The core starting a chat, which the pane then takes up by attaching once. */
    let started: object | null = null;
    const start = vi.fn(async () => ({ sessionId: "session-only", capabilities: {}, setup: {} }));
    return {
        acpApi: {
            subscribe: vi.fn(async () => () => {}),
            attach: vi.fn(async () => {
                const begun = started;
                started = null;
                return begun
                    ? {
                          status: "live",
                          start: begun,
                          permissionMode: "workspace-write",
                          running: false,
                          turned: false,
                          feed: "feed-1",
                          olderBefore: null,
                      }
                    : { status: "missing" };
            }),
            startChat: vi.fn(async () => {
                started = await start();
            }),
            setPermissionMode: vi.fn(async () => {}),
            stop: vi.fn(async () => {}),
            prompt: vi.fn(async () => {}),
            cancel: vi.fn(async () => {}),
            permissionReply: vi.fn(async () => {}),
        },
    };
});

vi.mock("../state/workspaceSent", () => ({ workspaceSent: async () => {} }));

vi.mock("../terminal/TerminalPane", () => ({
    TerminalPane: (props: ComponentProps<typeof TerminalPane>) => (
        <div data-testid={`terminal-${props.context?.agentId ?? "window"}`} data-visible={String(props.visible)} />
    ),
}));

const initial = getState();

const Stage = () => (
    <>
        <WorkspaceTabs />
        <Workspace />
    </>
);

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

/* The agent's pane loads lazily, which takes longer than a second while the whole suite runs at once. */
const LAZY_PANE_MS = 5_000;

describe("workspace tab bars", () => {
    it.each([
        ["one terminal", () => {}],
        ["one agent", () => projectWithAgent()],
    ])("keeps the tab bar and new-tab action visible with %s", (_, setup) => {
        setup();
        render(<Stage />);

        expect(screen.getByRole("tablist")).toBeInTheDocument();
        fireEvent.click(screen.getByRole("button", { name: "New tab" }));
        expect(getState().newTabPaletteOpen).toBe(true);
    });

    it("keeps the agent's tab in the strip once its desk takes focus", () => {
        projectWithAgent();
        render(<Stage />);
        expect(screen.getByRole("tab", { name: /only agent/ })).toBeInTheDocument();

        act(() => cmd.openDesk("agent-only"));

        // The desk is the focused pane now. Reading the agent off the focused
        // pane finds nothing and drops the tab, stranding the agent.
        expect(screen.getByRole("tab", { name: /only agent/ })).toBeInTheDocument();
    });

    it("updates permission mode from the session composer", async () => {
        projectWithAgent(false);
        render(<Stage />);

        const toggle = await screen.findByRole("button", { name: /safe/i }, { timeout: LAZY_PANE_MS });
        await waitFor(() => expect(toggle).not.toBeDisabled());
        fireEvent.click(toggle);

        expect(getState().agents["agent-only"]).toMatchObject({
            permissionMode: "bypass",
        });
        expect(await screen.findByRole("button", { name: /yolo/i }, { timeout: LAZY_PANE_MS })).toHaveAttribute("aria-pressed", "true");
        expect(screen.queryByTestId("terminal-agent-only")).not.toBeInTheDocument();
    });

    it("flips YOLO from the TUI view and relaunches the CLI", async () => {
        projectWithAgent(false);
        render(<Stage />);

        await act(async () => {
            fireEvent.click(await screen.findByRole("button", { name: "TUI" }, { timeout: LAZY_PANE_MS }));
        });

        const original = await screen.findByTestId("terminal-agent-only", {}, { timeout: LAZY_PANE_MS });
        const header = document.querySelector(".agent-surface-header") as HTMLElement;
        const toggle = within(header).getByRole("button", { name: /safe/i });
        expect(toggle).toHaveAttribute("aria-pressed", "false");
        await act(async () => {
            fireEvent.click(toggle);
        });

        expect(getState().agents["agent-only"]).toMatchObject({
            permissionMode: "bypass",
        });
        expect(toggle).toHaveTextContent("yolo");
        expect(toggle).toHaveAttribute("aria-pressed", "true");
        expect(screen.getByTestId("terminal-agent-only")).not.toBe(original);
    });

    it("keeps the ACP session alive while its tab is hidden", async () => {
        const sessionId = projectWithAgent(false);
        render(<Stage />);
        await waitFor(() => expect(acpApi.startChat).toHaveBeenCalledTimes(1));
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
        expect(acpApi.startChat).toHaveBeenCalledTimes(1);
    });

    it("changes the harness in place before the first message and preserves the draft", async () => {
        const sessionId = projectWithAgent(false);
        render(<Stage />);
        const editor = await screen.findByRole("textbox", { name: "Message agent" });
        await waitFor(() => expect(editor).toBeEnabled());
        typeDraft(editor, "Keep this draft");
        fireEvent.click(screen.getByRole("button", { name: "Model" }));
        fireEvent.click(within(screen.getByRole("group", { name: "Agent" })).getByRole("button", { name: /Claude/ }));
        await waitFor(() => expect(acpApi.startChat).toHaveBeenCalledTimes(2));
        expect(acpApi.startChat).toHaveBeenLastCalledWith("agent-only");
        expect(acpApi.stop).toHaveBeenCalledWith("agent-only");
        expect(agentIdsOf(getState(), sessionId)).toEqual(["agent-only"]);
        expect(getState().agents["agent-only"]).toMatchObject({ type: "claude", title: "Claude" });
        expect(draftOf(editor)).toBe("Keep this draft");
        expect(within(screen.getByRole("group", { name: "Agent" })).getByRole("button", { name: /Claude/ })).toHaveAttribute("aria-pressed", "true");
        expect(screen.queryByText("only agent")).not.toBeInTheDocument();
        expect(screen.getAllByText("Claude").length).toBeGreaterThan(0);
    });

    it("switches from an agent tab to a window tab through the same strip", () => {
        projectWithAgent();
        render(<Stage />);

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
        setState((s) => withAgents(s, sessionId, [{ id: "agent-two", type: "codex", title: "second agent", launchState: "live" }]));
    }

    /*
     * A document tab lighting no layer, or two layers at once, both read as the
     * editor and an agent painting over each other on one stage.
     */
    it("shows exactly one layer when a document tab is active alongside an agent", () => {
        const sessionId = getState().activeSessionId;
        addAgentTo(sessionId);
        cmd.requestOpenFile("/repo/a.ts");

        render(<Stage />);

        expect(document.querySelectorAll(".window-layer.live")).toHaveLength(1);
    });

    it("still renders the editor layer when it holds no document", () => {
        cmd.openEditorPane();

        render(<Stage />);

        expect(document.querySelectorAll(".window-layer.live")).toHaveLength(1);
    });
});

describe("naming a terminal tab", () => {
    function terminalWindow() {
        const state = getState();
        const id = state.windowsBySession[state.activeSessionId].find((winId) => state.windows[winId].role === "term")!;
        return state.windows[id];
    }

    it("shows the given name over the terminal's own title, and can go back to it", async () => {
        const win = terminalWindow();
        cmd.setPanePty(win.activePaneId, 7);
        setState({ terminalTitles: { 7: "zsh" } });
        render(<Stage />);
        expect(screen.getByRole("tab", { name: "zsh" })).toBeInTheDocument();

        fireEvent.doubleClick(screen.getByRole("tab", { name: "zsh" }));
        const dialog = useDialogs.getState().dialog!;
        expect(dialog).toMatchObject({ kind: "prompt", title: "Rename tab", initial: "zsh" });
        act(() => acceptDialog(dialog.id, "  api server  "));

        expect(await screen.findByRole("tab", { name: "api server" })).toBeInTheDocument();
        expect(getState().windows[win.id].customName).toBe("api server");

        act(() => setState({ terminalTitles: { 7: "vim" } }));
        expect(screen.getByRole("tab", { name: "api server" })).toBeInTheDocument();

        fireEvent.contextMenu(screen.getByRole("tab", { name: "api server" }));
        fireEvent.click(screen.getByText("Use Automatic Name"));
        expect(screen.getByRole("tab", { name: "vim" })).toBeInTheDocument();
        expect(getState().windows[win.id].customName).toBeUndefined();
    });

    it("renames an agent's tab the same way, by double-clicking it", () => {
        projectWithAgent();
        render(<Stage />);

        fireEvent.doubleClick(screen.getByRole("tab", { name: /only agent/ }));

        expect(useDialogs.getState().dialog).toMatchObject({ kind: "prompt", title: "Edit title", initial: "only agent" });
    });

    it("keeps the name a duplicated tab was given", () => {
        const win = terminalWindow();
        cmd.renameWindow(win.id, "logs");
        cmd.duplicateWindow(win.id);

        const names = Object.values(getState().windows).map((w) => w.customName);
        expect(names).toContain("logs");
        expect(names).toContain("logs copy");
    });
});
