import { acpApi, type AcpChat } from "../api/acp";
import { coreSessionsApi, type CoreSession } from "../api/coreSessions";
import { chatClaimed } from "../chat/chatClaims";
import * as cmd from "../state/commands";
import { agentSessionPlan, claimedSessionIds, unclaimedTerminals } from "../state/coreSessionClaims";
import { getState, setState } from "../state/store";
import { notify } from "../state/toast";
import { adoptCoreTasks, type TaskAdoptionTargets } from "../tasks/adoption";
import { appTaskRuntime } from "../tasks/application";
import { NativeTaskExecutionBackend } from "../tasks/nativeRuntime";
import { offerResumableSessions, spawnedThisPage } from "../terminal/sessionResume";
import { relaunchTuiAgent } from "../agents/tuiResume";

/** Long enough for every pane of a restored layout to have taken its terminal back. */
export const UNCLAIMED_GRACE_MS = 30_000;

export const KEPT_RUNNING_NOTICE =
    "Your terminals and agents kept running while Sikemux was closed. Use Quit and Stop Everything (⌥⌘Q) to stop them.";

/** Call right after the saved layout is applied, before any pane mounts. */
export function offerSavedSessions(): void {
    offerResumableSessions(claimedSessionIds(getState()));
}

export interface CoreSessionRestoreDeps {
    list(): Promise<CoreSession[]>;
    resume(agentId: string, ptyId: number): Promise<void>;
    kill(id: number): Promise<void>;
    chats: { list(): Promise<AcpChat[]>; stop(agentId: string): Promise<void> };
    tasks: TaskAdoptionTargets;
    schedule(callback: () => void, delayMs: number): void;
}

function defaultDeps(): CoreSessionRestoreDeps {
    const backend = new NativeTaskExecutionBackend();
    return {
        list: coreSessionsApi.list,
        resume: relaunchTuiAgent,
        kill: coreSessionsApi.kill,
        chats: { list: acpApi.list, stop: acpApi.stop },
        tasks: {
            watch: (ptyId) => backend.watch(ptyId),
            adoptDeckTask: (task, executionId, started) => appTaskRuntime.adopt(task, executionId, started),
            showHarnessTerminal: (request) =>
                void import("../harness/service")
                    .then(({ harnessTerminals }) => harnessTerminals.open({ ...request, background: true, signal: new AbortController().signal }))
                    .catch(() => {}),
        },
        schedule: (callback, delayMs) => void window.setTimeout(callback, delayMs),
    };
}

/**
 * Takes back what the core kept while the app was closed or reloading:
 * terminal agents still running come back live, ones that crashed meanwhile
 * are resumed in their pane, running tasks rejoin the
 * command deck or reopen their terminals, chat panes take their chats back
 * as they mount, and terminals from before this page that nothing in the
 * layout names, like chats no pane took back, are stopped after a grace, so
 * none of them runs forever unseen.
 */
export async function restoreCoreSessions(deps: CoreSessionRestoreDeps = defaultDeps()): Promise<void> {
    let sessions: CoreSession[];
    try {
        sessions = await deps.list();
    } catch {
        return;
    }
    const chats = await deps.chats.list().catch((): AcpChat[] => []);
    for (const chat of chats) if (chat.startedBy !== null) cmd.adoptChat(chat);
    const plan = agentSessionPlan(getState(), sessions);
    cmd.applyAgentSessionPlan(plan);
    for (const id of plan.resume) {
        const ptyId = getState().agents[id]?.ptyId;
        if (ptyId !== undefined) void deps.resume(id, ptyId).catch(() => {});
    }
    const tasks = await adoptCoreTasks(sessions, deps.tasks);

    const claimed = claimedSessionIds(getState());
    const earlier = sessions.filter((session) => !spawnedThisPage(session.id));
    const terminals = earlier.filter((session) => session.kind === "terminal" && session.running && claimed.has(session.id)).length;
    const agents = getState().agents;
    const keptChats = chats.filter((chat) => agents[chat.agentId]).length;
    if (terminals + tasks + keptChats > 0 && !getState().keptRunningNoticeShown) {
        setState({ keptRunningNoticeShown: true });
        notify("info", KEPT_RUNNING_NOTICE, { timeoutMs: 12_000 });
    }

    const candidates = unclaimedTerminals(earlier, claimed);
    if (candidates.length === 0 && chats.length === 0) return;
    deps.schedule(() => {
        const stillClaimed = claimedSessionIds(getState());
        for (const id of candidates) if (!stillClaimed.has(id)) void deps.kill(id).catch(() => {});
        for (const chat of chats) if (chat.startedBy === null && !chatClaimed(chat.agentId)) void deps.chats.stop(chat.agentId).catch(() => {});
    }, UNCLAIMED_GRACE_MS);
}
