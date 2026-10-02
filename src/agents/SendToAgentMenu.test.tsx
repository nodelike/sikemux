import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ available: vi.fn(), send: vi.fn(() => true) }));

vi.mock("../api/agents", () => ({ agentApi: { available: mocks.available } }));
vi.mock("./sendToAgent", () => ({ sendToAgent: mocks.send }));

import { getState, setState } from "../state/store";
import { SendToAgentMenu } from "./SendToAgentMenu";

const initial = getState();

beforeEach(() => {
    setState(initial, true);
    const pane = { type: "pane" as const, id: "a1", cwd: "/repo", kind: "agent" as const, title: "Fix it" };
    setState({
        sessions: { p: { id: "p", name: "repo", kind: "project", cwd: "/repo", pinned: false, activeWindowId: "w" } },
        sessionOrder: ["p"],
        activeSessionId: "p",
        windows: { w: { id: "w", name: "Fix it", role: "agent", root: pane, activePaneId: "a1" } },
        windowsBySession: { p: ["w"] },
        agents: { a1: { id: "a1", type: "codex", title: "Fix it", startup: "" } },
    });
    mocks.available.mockResolvedValue([{ type: "claude", label: "Claude", command: "claude", defaultModel: null, defaultEffort: null }]);
    mocks.send.mockClear();
});

afterEach(cleanup);

describe("SendToAgentMenu", () => {
    it("offers the open agent and a new chat, and hands the delivery to the one chosen", async () => {
        const onClose = vi.fn();
        render(<SendToAgentMenu x={10} y={10} delivery={() => ({ text: "hello" })} onClose={onClose} />);
        expect(screen.getByRole("menuitem", { name: "Fix it" })).toBeTruthy();
        fireEvent.click(await screen.findByRole("menuitem", { name: "New Claude chat" }));
        expect(mocks.send).toHaveBeenCalledWith({ newAgent: "claude", sessionId: "p" }, { text: "hello" });
        expect(onClose).toHaveBeenCalled();
    });

    it("says so when there is nobody to send to", () => {
        render(<SendToAgentMenu x={0} y={0} sessionId={null} delivery={() => ({ text: "x" })} onClose={() => {}} />);
        expect((screen.getByRole("menuitem", { name: "No agents available" }) as HTMLButtonElement).disabled).toBe(true);
    });
});
