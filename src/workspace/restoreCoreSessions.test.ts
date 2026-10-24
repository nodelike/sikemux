import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import type { ChatInfo, SessionInfo } from "../api/generated/wire";
import { sessionInfo } from "../test/wire";
import * as cmd from "../state/commands";
import { getState, setState } from "../state/store";
import { useToasts } from "../state/toast";
import { agentWindowId } from "../state/selectors";
import { withAgents } from "../test/agents";
import { isResumableSession, noteSpawnedSession, takeResumableSession } from "../terminal/sessionResume";
import { KEPT_RUNNING_NOTICE, UNCLAIMED_GRACE_MS, offerSavedSessions, restoreCoreSessions, type CoreSessionRestoreDeps } from "./restoreCoreSessions";
import { testPreferences } from "../test/preferences";

const initial = getState();

function terminal(id: number, running = true, exit: SessionInfo["exit"] = running ? null : { code: 0, signal: null }): SessionInfo {
    return sessionInfo({ id, running, exit });
}

function deps(sessions: SessionInfo[], chats: ChatInfo[] = []) {
    const scheduled: Array<() => void> = [];
    const kill = vi.fn(async (_id: number) => {});
    const restore: CoreSessionRestoreDeps = {
        list: async () => sessions,
        kill,
        chats: async () => chats,
        schedule: (callback, delay) => {
            expect(delay).toBe(UNCLAIMED_GRACE_MS);
            scheduled.push(callback);
        },
    };
    return { restore, kill, runScheduled: () => scheduled.splice(0).forEach((callback) => callback()) };
}

function layoutWithSessions() {
    const sid = getState().activeSessionId;
    const paneId = getState().windows[getState().sessions[sid].activeWindowId].activePaneId;
    cmd.setPanePty(paneId, 101);
    setState((s) => {
        const slices = withAgents(s, sid, [
            { id: "agent-live", type: "pi", title: "live", ptyId: 102, launchState: "dormant" },
            { id: "agent-ended", type: "claude", title: "ended", resumeId: "r1", ptyId: 103, launchState: "dormant" },
        ]);
        return { ...slices, sessions: { ...s.sessions, [sid]: { ...s.sessions[sid], kind: "project" } } };
    });
    return { sid, paneId };
}

beforeEach(() => {
    setState(initial, true);
    setState({ preferences: testPreferences() });
    useToasts.setState({ toasts: [] });
});

describe("restoring what the core kept", () => {
    it("lets each saved terminal be taken back once", () => {
        layoutWithSessions();
        offerSavedSessions();
        expect(isResumableSession(101)).toBe(true);
        expect(takeResumableSession(102)).toBe(true);
        expect(takeResumableSession(102)).toBe(false);
        expect(isResumableSession(999)).toBe(false);
        takeResumableSession(101);
        takeResumableSession(103);
    });

    it("shows live the agents whose terminal runs, leaves the others asleep, and stops only unclaimed terminals after the grace", async () => {
        const { sid } = layoutWithSessions();
        const { restore, kill, runScheduled } = deps([terminal(101), terminal(102), terminal(103, false), terminal(104), terminal(105)]);

        await restoreCoreSessions(restore);

        expect(getState().agents["agent-live"]).toMatchObject({ launchState: "live", ptyId: 102 });
        expect(getState().agents["agent-ended"].launchState).toBe("dormant");
        expect(agentWindowId(getState(), "agent-ended")).toBeTruthy();
        expect(kill).not.toHaveBeenCalled();

        cmd.newWindow();
        cmd.setPanePty(getState().windows[getState().sessions[sid].activeWindowId].activePaneId, 104);
        runScheduled();
        expect(kill.mock.calls.map(([id]) => id).sort()).toEqual([105]);
    });

    it("says once that terminals kept running", async () => {
        layoutWithSessions();
        await restoreCoreSessions(deps([terminal(101)]).restore);
        expect(useToasts.getState().toasts.map((toast) => toast.text)).toEqual([KEPT_RUNNING_NOTICE]);
        expect(getState().preferences?.keptRunningNoticeShown).toBe(true);

        useToasts.setState({ toasts: [] });
        await restoreCoreSessions(deps([terminal(101)]).restore);
        expect(useToasts.getState().toasts).toEqual([]);
    });

    it("says nothing when nothing came back", async () => {
        layoutWithSessions();
        await restoreCoreSessions(deps([terminal(104)]).restore);
        expect(useToasts.getState().toasts).toEqual([]);
        expect(getState().preferences?.keptRunningNoticeShown).toBe(false);
    });

    it("leaves alone a terminal this page started, such as a popup opened right after launch", async () => {
        noteSpawnedSession(201);
        const { restore, kill, runScheduled } = deps([terminal(201)]);
        await restoreCoreSessions(restore);
        runScheduled();
        expect(kill).not.toHaveBeenCalled();
        expect(useToasts.getState().toasts).toEqual([]);
    });
});
