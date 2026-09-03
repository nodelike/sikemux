import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ComponentProps } from "react";
import type { TerminalPane } from "../terminal/TerminalPane";
import { getState, setState } from "../state/store";
import { Workspace } from "./Workspace";

vi.mock("../api/acp", () => ({
    acpApi: {
        subscribe: vi.fn(async () => () => {}),
        start: vi.fn(() => new Promise(() => {})),
        stop: vi.fn(async () => {}),
        prompt: vi.fn(async () => {}),
        cancel: vi.fn(async () => {}),
        permissionReply: vi.fn(async () => {}),
    },
}));

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
    setState({
        sessions: {
            ...state.sessions,
            [sessionId]: { ...session, kind: "project", view: "agent", activeAgentId: "agent-visible", cwd: "/repo" },
        },
        agents: {
            "agent-visible": {
                id: "agent-visible",
                type: "codex",
                title: "visible",
                startup: "codex resume visible-session",
                resumeId: "visible-session",
                launchState: "live",
            },
            "agent-hidden": {
                id: "agent-hidden",
                type: "claude",
                title: "hidden",
                startup: "claude --resume hidden-session",
                resumeId,
                launchState: "live",
            },
        },
        agentsBySession: { ...state.agentsBySession, [sessionId]: ["agent-visible", "agent-hidden"] },
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

    it("resumes a sleeping agent into Session and starts TUI only after switching", async () => {
        arrangeRestoredAgents("hidden-session");
        setState((state) => ({
            agents: {
                ...state.agents,
                "agent-visible": { ...state.agents["agent-visible"], launchState: "dormant" },
            },
        }));
        render(<Workspace />);

        expect(screen.queryByTestId("terminal-agent-visible")).not.toBeInTheDocument();
        fireEvent.click(screen.getByRole("button", { name: "Resume codex" }));
        expect(screen.queryByTestId("terminal-agent-visible")).not.toBeInTheDocument();
        fireEvent.click((await screen.findAllByRole("button", { name: "TUI" }))[0]);
        await waitFor(() => expect(screen.getByTestId("terminal-agent-visible")).toHaveAttribute("data-spawn-when", "true"));
    });
});
