import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import type { AcpChat } from "../api/acp";
import type { CoreSession } from "../api/coreSessions";
import { claimChat } from "../chat/chatClaims";
import * as cmd from "../state/commands";
import { getState, setState } from "../state/store";
import { useToasts } from "../state/toast";
import { agentWindowId } from "../state/selectors";
import { withAgents } from "../test/agents";
import { isResumableSession, noteSpawnedSession, takeResumableSession } from "../terminal/sessionResume";
import { KEPT_RUNNING_NOTICE, UNCLAIMED_GRACE_MS, offerSavedSessions, restoreCoreSessions, type CoreSessionRestoreDeps } from "./restoreCoreSessions";

const initial = getState();

function terminal(id: number, running = true, exit: CoreSession["exit"] = running ? null : { code: 0, signal: null }): CoreSession {
    return {
        id,
        kind: "terminal",
        pid: 1,
        running,
        project: null,
        paneId: null,
        agentId: null,
        agentType: null,
        task: null,
        exit,
        killed: false,
        startedBy: null,
    };
}

function chat(agentId: string, startedBy: string | null = null): AcpChat {
    return {
        agentId,
        provider: "claude",
        cwd: "/repo",
        sessionId: "s",
        state: "ready",
        running: true,
        pendingPermissions: [],
        startedBy,
        launcher: startedBy ? "claude" : null,
        permissionMode: "bypass",
        model: null,
        effort: null,
    };
}

function deps(sessions: CoreSession[], chats: AcpChat[] = []) {
    const scheduled: Array<() => void> = [];
    const kill = vi.fn(async (_id: number) => {});
    const stopChat = vi.fn(async (_agentId: string) => {});
    const resume = vi.fn(async (_agentId: string, _ptyId: number) => {});
    const restore: CoreSessionRestoreDeps = {
        list: async () => sessions,
        resume,
        kill,
        chats: { list: async () => chats, stop: stopChat },
        tasks: { watch: vi.fn(), adoptDeckTask: vi.fn(), showHarnessTerminal: vi.fn() },
        schedule: (callback, delay) => {
            expect(delay).toBe(UNCLAIMED_GRACE_MS);
            scheduled.push(callback);
        },
    };
    return { restore, kill, stopChat, resume, runScheduled: () => scheduled.splice(0).forEach((callback) => callback()) };
}

function layoutWithSessions() {
    const sid = getState().activeSessionId;
    const paneId = getState().windows[getState().sessions[sid].activeWindowId].activePaneId;
    cmd.setPanePty(paneId, 101);
    setState((s) => {
        const slices = withAgents(s, sid, [
            { id: "agent-live", type: "pi", title: "live", startup: "pi", ptyId: 102, launchState: "dormant" },
            { id: "agent-ended", type: "claude", title: "ended", startup: "claude", resumeId: "r1", ptyId: 103, launchState: "dormant" },
        ]);
        return { ...slices, sessions: { ...s.sessions, [sid]: { ...s.sessions[sid], kind: "project" } } };
    });
    return { sid, paneId };
}

beforeEach(() => {
    setState(initial, true);
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

    it("wakes agents whose terminal runs, lets the others sleep, and stops only unclaimed terminals after the grace", async () => {
        const { sid } = layoutWithSessions();
        const { restore, kill, runScheduled } = deps([terminal(101), terminal(102), terminal(103, false), terminal(104), terminal(105)]);

        await restoreCoreSessions(restore);

        expect(getState().agents["agent-live"]).toMatchObject({ launchState: "live", ptyId: 102 });
        expect(getState().agents["agent-ended"].launchState).toBe("dormant");
        expect(getState().agents["agent-ended"].ptyId).toBeUndefined();
        expect(agentWindowId(getState(), "agent-ended")).toBeTruthy();
        expect(kill).not.toHaveBeenCalled();

        cmd.newWindow();
        cmd.setPanePty(getState().windows[getState().sessions[sid].activeWindowId].activePaneId, 104);
        runScheduled();
        expect(kill.mock.calls.map(([id]) => id).sort()).toEqual([103, 105]);
    });

    it("resumes an agent whose terminal crashed while the app was closed, in the pane that showed it", async () => {
        layoutWithSessions();
        offerSavedSessions();
        const { restore, resume, kill, runScheduled } = deps([terminal(101), terminal(102), terminal(103, false, { code: 1, signal: "Killed: 9" })]);

        await restoreCoreSessions(restore);

        expect(getState().agents["agent-ended"]).toMatchObject({ launchState: "live", ptyId: 103 });
        expect(isResumableSession(103)).toBe(true);
        expect(resume).toHaveBeenCalledExactlyOnceWith("agent-ended", 103);
        runScheduled();
        expect(kill).not.toHaveBeenCalled();
        for (const id of [101, 102, 103]) takeResumableSession(id);
    });

    it("lets an agent the person quit sleep instead of resuming it", async () => {
        layoutWithSessions();
        const { restore, resume } = deps([terminal(101), terminal(102), terminal(103, false, { code: 0, signal: null })]);
        await restoreCoreSessions(restore);
        expect(resume).not.toHaveBeenCalled();
        expect(getState().agents["agent-ended"].launchState).toBe("dormant");
    });

    it("closes a terminal agent with nothing to resume once its terminal is gone", async () => {
        const sid = getState().activeSessionId;
        setState((s) => {
            const slices = withAgents(s, sid, [{ id: "agent-gone", type: "pi", title: "gone", startup: "pi", ptyId: 7 }]);
            return { ...slices, sessions: { ...s.sessions, [sid]: { ...s.sessions[sid], kind: "project" } } };
        });
        await restoreCoreSessions(deps([]).restore);
        expect(getState().agents["agent-gone"]).toBeUndefined();
    });

    it("says once that terminals kept running", async () => {
        layoutWithSessions();
        await restoreCoreSessions(deps([terminal(101)]).restore);
        expect(useToasts.getState().toasts.map((toast) => toast.text)).toEqual([KEPT_RUNNING_NOTICE]);
        expect(getState().keptRunningNoticeShown).toBe(true);

        useToasts.setState({ toasts: [] });
        await restoreCoreSessions(deps([terminal(101)]).restore);
        expect(useToasts.getState().toasts).toEqual([]);
    });

    it("says nothing when nothing came back", async () => {
        layoutWithSessions();
        await restoreCoreSessions(deps([terminal(104)]).restore);
        expect(useToasts.getState().toasts).toEqual([]);
        expect(getState().keptRunningNoticeShown).toBe(false);
    });

    it("stops the chats no chat pane took back after the grace", async () => {
        layoutWithSessions();
        const { restore, stopChat, runScheduled } = deps(
            [],
            [chat("agent-ended"), chat("agent-shown"), chat("agent-forgotten"), chat("agent-phone", "phone-key")],
        );
        await restoreCoreSessions(restore);
        expect(useToasts.getState().toasts.map((toast) => toast.text)).toEqual([KEPT_RUNNING_NOTICE]);
        claimChat("agent-shown");
        expect(stopChat).not.toHaveBeenCalled();
        runScheduled();
        expect(stopChat.mock.calls.map(([id]) => id).sort()).toEqual(["agent-ended", "agent-forgotten"]);
    });

    it("shows a chat a phone started among its project's agents without switching to it", async () => {
        layoutWithSessions();
        const before = getState().activeSessionId;
        const { restore } = deps([], [{ ...chat("agent-phone", "phone-key"), cwd: "/elsewhere", model: "opus" }]);
        await restoreCoreSessions(restore);
        const state = getState();
        expect(state.agents["agent-phone"]).toMatchObject({
            type: "claude",
            cwd: "/elsewhere",
            resumeId: "s",
            permissionMode: "bypass",
            model: "opus",
        });
        expect(state.activeSessionId).toBe(before);
        const project = Object.values(state.sessions).find((session) => session.cwd === "/elsewhere");
        expect(project?.kind).toBe("project");
        expect(state.windowsBySession[project?.id ?? ""]).toContain(agentWindowId(state, "agent-phone"));
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
