import { act, cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { agentApi } from "../api/agents";
import { installIpcTransportForTests, MemoryIpcTransport, resetIpcTransportForTests } from "../api/transport";
import * as resources from "../state/resources";
import { getState, setState } from "../state/store";
import { AgentSessionSync } from "./AgentSessionSync";

const initial = getState();
let transport: MemoryIpcTransport;

beforeEach(() => {
    setState(initial, true);
    const state = getState();
    const sessionId = state.activeSessionId;
    setState({
        sessions: {
            ...state.sessions,
            [sessionId]: {
                ...state.sessions[sessionId],
                kind: "project",
                cwd: "/repo",
                view: "windows",
                activeAgentId: "agent-1",
            },
        },
        agents: {
            "agent-1": { id: "agent-1", type: "codex", title: "Codex", startup: "codex", cwd: "/repo" },
        },
        agentsBySession: { ...state.agentsBySession, [sessionId]: ["agent-1"] },
    });
    resetIpcTransportForTests();
    transport = new MemoryIpcTransport();
    installIpcTransportForTests(transport);
    vi.spyOn(resources, "fetchResource").mockImplementation(() => new Promise<never>(() => {}));
    vi.spyOn(agentApi, "watchStart").mockResolvedValue(17);
    vi.spyOn(agentApi, "watchStop").mockResolvedValue(undefined);
});

afterEach(() => {
    cleanup();
    resetIpcTransportForTests();
    vi.restoreAllMocks();
    setState(initial, true);
});

describe("AgentSessionSync IPC events", () => {
    it("routes both agent event streams and aborts their transport subscriptions", async () => {
        const view = render(<AgentSessionSync />);
        await waitFor(() => expect(transport.eventListenerCount).toBe(2));
        await waitFor(() => expect(resources.fetchResource).toHaveBeenCalledOnce());

        act(() => {
            transport.emit("agent_state_changed", {
                agentId: "agent-1",
                state: "working",
                sequence: 1,
                source: "process",
                confidence: "high",
                reason: "process is running",
            });
        });
        expect(getState().agentActivity["agent-1"]).toMatchObject({ backendState: "working", sequence: 1 });

        act(() => {
            transport.emit("agent_sessions_changed", { agent: "codex", cwd: "/repo" });
        });
        await waitFor(() => expect(resources.fetchResource).toHaveBeenCalledTimes(2));

        view.unmount();
        expect(transport.eventListenerCount).toBe(0);
        await waitFor(() => expect(agentApi.watchStop).toHaveBeenCalledWith(17));
    });

    it("does not watch transcript directories for sleeping agents", async () => {
        setState((state) => ({
            agents: {
                ...state.agents,
                "agent-1": { ...state.agents["agent-1"], resumeId: "session-1", launchState: "dormant" },
            },
        }));

        render(<AgentSessionSync />);
        await waitFor(() => expect(transport.eventListenerCount).toBe(2));
        expect(resources.fetchResource).not.toHaveBeenCalled();
        expect(agentApi.watchStart).not.toHaveBeenCalled();
    });

    it("reconciles watcher groups without restarting unchanged watches", async () => {
        vi.mocked(agentApi.watchStart).mockResolvedValueOnce(17).mockResolvedValueOnce(18);
        render(<AgentSessionSync />);
        await waitFor(() => expect(agentApi.watchStart).toHaveBeenCalledTimes(1));

        const sessionId = getState().activeSessionId;
        act(() =>
            setState((state) => ({
                agents: {
                    ...state.agents,
                    "agent-2": { id: "agent-2", type: "codex", title: "Other", startup: "codex", cwd: "/other" },
                },
                agentsBySession: { ...state.agentsBySession, [sessionId]: ["agent-1", "agent-2"] },
            })),
        );

        await waitFor(() => expect(agentApi.watchStart).toHaveBeenCalledTimes(2));
        expect(agentApi.watchStop).not.toHaveBeenCalled();

        act(() =>
            setState((state) => ({
                agents: { ...state.agents, "agent-1": { ...state.agents["agent-1"], launchState: "dormant" } },
            })),
        );
        await waitFor(() => expect(agentApi.watchStop).toHaveBeenCalledWith(17));
        expect(agentApi.watchStop).not.toHaveBeenCalledWith(18);
    });

    it("keeps session discovery inside the selected provider profile", async () => {
        setState((state) => ({
            providerProfiles: [{ id: "codex-work", name: "Codex Work", provider: "codex", accent: "#10a37f", configPath: "~/.codex-work" }],
            agents: { ...state.agents, "agent-1": { ...state.agents["agent-1"], profileId: "codex-work" } },
        }));

        render(<AgentSessionSync />);

        await waitFor(() => expect(resources.fetchResource).toHaveBeenCalledWith(expect.anything(), "codex", "/repo", "~/.codex-work"));
        await waitFor(() => expect(agentApi.watchStart).toHaveBeenCalledWith("codex", "/repo", "~/.codex-work"));
    });
});
