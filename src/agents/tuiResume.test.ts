import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { getState, setState } from "../state/store";
import type { Agent } from "../state/types";
import { withAgents } from "../test/agents";
import { isResumableSession, spawnedThisPage, takeResumableSession } from "../terminal/sessionResume";
import { RESUME_WINDOW_MS } from "../chat/sessionRecovery";
import { noteTerminalExit, relaunchTuiAgent, resetTuiResumeForTests, tuiResumeView, type TuiResumeDeps, type TuiSpawnRequest } from "./tuiResume";

const initial = getState();
const crash = { code: 1, signal: "Killed: 9", killed: false };

function setup(overrides: Partial<Agent> = {}) {
    const sid = getState().activeSessionId;
    const agent: Agent = {
        id: "agent-tui",
        type: "claude",
        title: "Claude",
        startup: "claude --resume r1",
        directCommand: { program: "claude", args: ["--resume", "r1"] },
        resumeId: "r1",
        permissionMode: "bypass",
        cwd: "/repo",
        ptyId: 40,
        launchState: "live",
        ...overrides,
    };
    setState((s) => {
        const slices = withAgents(s, sid, [agent]);
        return { ...slices, sessions: { ...s.sessions, [sid]: { ...s.sessions[sid], kind: "project", name: "repo" } } };
    });
    return sid;
}

function fakeDeps(start = 1_000_000) {
    let clock = start;
    let next = 50;
    const requests: TuiSpawnRequest[] = [];
    const deps: TuiResumeDeps = {
        spawn: vi.fn(async (request: TuiSpawnRequest) => {
            requests.push(request);
            return next++;
        }),
        now: () => clock,
    };
    return { deps, requests, advance: (ms: number) => (clock += ms) };
}

beforeEach(() => {
    setState(initial, true);
    resetTuiResumeForTests();
});

describe("a terminal agent that dies", () => {
    it("comes back on its saved conversation as a new terminal that carries the old screen, in the same pane", async () => {
        const sid = setup();
        const { deps, requests } = fakeDeps();

        await noteTerminalExit(40, crash, deps);

        expect(requests).toEqual([
            {
                cols: 80,
                rows: 24,
                cwd: "/repo",
                startup: null,
                directCommand: { program: "claude", args: ["--resume", "r1"] },
                context: { sessionId: sid, sessionName: "repo", sessionKind: "project", project: "/repo", agentId: "agent-tui", agentType: "claude" },
                continues: { session: 40, note: "— Resuming Claude —" },
            },
        ]);
        expect(getState().agents["agent-tui"].ptyId).toBe(50);
        expect(spawnedThisPage(50)).toBe(true);
        expect(isResumableSession(50)).toBe(true);
        expect(tuiResumeView("agent-tui")).toMatchObject({ recovery: null, generation: 1 });
        takeResumableSession(50);
    });

    it("says it is resuming until the new terminal has started", async () => {
        setup();
        let started!: (id: number) => void;
        const deps: TuiResumeDeps = { spawn: () => new Promise((resolve) => (started = resolve)), now: () => 0 };
        const resuming = noteTerminalExit(40, crash, deps);
        await Promise.resolve();
        expect(tuiResumeView("agent-tui").recovery).toEqual({ phase: "resuming" });
        started(60);
        await resuming;
        expect(tuiResumeView("agent-tui").recovery).toBeNull();
        takeResumableSession(60);
    });

    it("gives up when it dies again within 30 seconds of a resume, and resumes again after that", async () => {
        setup();
        const { deps, requests, advance } = fakeDeps();
        await noteTerminalExit(40, crash, deps);
        advance(RESUME_WINDOW_MS - 1);
        await noteTerminalExit(50, { code: 1, signal: null, killed: false }, deps);
        expect(requests).toHaveLength(1);
        expect(tuiResumeView("agent-tui").recovery).toEqual({ phase: "failed", detail: "The agent exited with code 1." });

        advance(1);
        await relaunchTuiAgent("agent-tui", 50, deps);
        advance(RESUME_WINDOW_MS);
        await noteTerminalExit(51, crash, deps);
        expect(requests.map((request) => request.continues?.session)).toEqual([40, 50, 51]);
        expect(tuiResumeView("agent-tui").recovery).toBeNull();
        for (const id of [50, 51, 52]) takeResumableSession(id);
    });

    it("gives up when the relaunch fails", async () => {
        setup();
        const deps: TuiResumeDeps = { spawn: async () => Promise.reject(new Error("claude: not found")), now: () => 0 };
        await noteTerminalExit(40, crash, deps);
        expect(tuiResumeView("agent-tui").recovery).toEqual({ phase: "failed", detail: "claude: not found" });
        expect(getState().agents["agent-tui"].ptyId).toBe(40);
    });

    it("stays down when Sikemux ended it, the person quit it, or it has no conversation to resume", async () => {
        setup();
        const { deps } = fakeDeps();
        await noteTerminalExit(40, { code: 1, signal: "Hangup", killed: true }, deps);
        await noteTerminalExit(40, { code: 0, signal: null, killed: false }, deps);
        await noteTerminalExit(40, { code: 1, signal: "Interrupt: 2", killed: false }, deps);
        await noteTerminalExit(99, crash, deps);
        setState(initial, true);
        setup({ resumeId: undefined });
        await noteTerminalExit(40, crash, deps);
        setState(initial, true);
        setup({ launchState: "dormant" });
        await noteTerminalExit(40, crash, deps);
        expect(deps.spawn).not.toHaveBeenCalled();
        expect(tuiResumeView("agent-tui")).toEqual({ recovery: null, generation: 0 });
    });
});
