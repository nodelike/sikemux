import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ComponentProps } from "react";
import type { TerminalPane } from "../terminal/TerminalPane";
import { getState, setState } from "../state/store";
import { Workspace } from "./Workspace";
import { agentWindowId } from "../state/selectors";
import { withAgents } from "../test/agents";

vi.mock("../api/acp", () => {
    /* The core starting a chat, which the pane then takes up by attaching once. */
    let started: object | null = null;
    const start = vi.fn(() => new Promise<never>(() => {}));
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
        <div
            data-testid={`terminal-${props.context?.agentId ?? "window"}`}
            data-visible={String(props.visible)}
            data-spawn-when={String(props.spawnWhen)}
        />
    ),
}));

const initial = getState();

beforeEach(() => setState(initial, true));
afterEach(cleanup);

function arrangeRestoredAgents(resumeId: string | undefined): void {
    const state = getState();
    const sessionId = state.activeSessionId;
    const session = state.sessions[sessionId];
    const slices = withAgents(state, sessionId, [
        {
            id: "agent-visible",
            type: "codex",
            title: "visible",
            resumeId: "visible-session",
            launchState: "live",
        },
        { id: "agent-hidden", type: "claude", title: "hidden", resumeId, launchState: "live" },
    ]);
    setState({
        ...slices,
        sessions: {
            ...state.sessions,
            [sessionId]: { ...session, kind: "project", cwd: "/repo", activeWindowId: agentWindowId(slices, "agent-visible")! },
        },
    });
}

describe("restored agent lifecycle", () => {
    it("does not start a hidden agent TUI", () => {
        arrangeRestoredAgents("hidden-session");
        render(<Workspace />);

        expect(screen.queryByTestId("terminal-agent-hidden")).not.toBeInTheDocument();
    });

    it("keeps hidden new agents out of the TUI", () => {
        arrangeRestoredAgents(undefined);
        render(<Workspace />);

        expect(screen.queryByTestId("terminal-agent-hidden")).not.toBeInTheDocument();
    });

    it("wakes a sleeping agent into GUI once it is shown, and starts TUI only after switching", async () => {
        arrangeRestoredAgents("hidden-session");
        setState((state) => ({
            agents: {
                ...state.agents,
                "agent-visible": { ...state.agents["agent-visible"], launchState: "dormant" },
            },
        }));
        render(<Workspace />);

        expect(screen.queryByTestId("terminal-agent-visible")).not.toBeInTheDocument();
        fireEvent.click((await screen.findAllByRole("button", { name: "TUI" }, { timeout: 5000 }))[0]);
        await waitFor(() => expect(screen.getByTestId("terminal-agent-visible")).toHaveAttribute("data-spawn-when", "true"));
    });
});
