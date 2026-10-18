import { acpApi, type AcpChat } from "../api/acp";
import { agentBoardApi } from "../api/agentBoard";
import { coreSessionsApi, type CoreSession } from "../api/coreSessions";
import { runOfAgent } from "../agents/agentBoard";
import { chatClaimed } from "../chat/chatClaims";
import { QUIT_AND_STOP_SHORTCUT } from "../commands/keybindings";
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

export const KEPT_RUNNING_NOTICE = `Your terminals and agents kept running while Sikemux was closed. Use Quit and Stop Everything${
    QUIT_AND_STOP_SHORTCUT ? ` (${QUIT_AND_STOP_SHORTCUT})` : ""
} to stop them.`;

/** Call right after the saved layout is applied, before any pane mounts. */
export function offerSavedSessions(): void {
    offerResumableSessions(claimedSessionIds(getState()));
}

export interface CoreSessionRestoreDeps {
    list(): Promise<CoreSession[]>;
    resume(agentId: string, ptyId: number): Promise<void>;
    kill(id: number): Promise<void>;
    chats: { list(): Promise<AcpChat[]>; stop(agentId: string): Promise<void> };
    boards(): Promise<void>;
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
        boards: () => agentBoardApi.all().then(cmd.setAgentBoards),
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
 * terminal agents still running come back live, as do chats other agents or a board's Auto started, ones that crashed meanwhile
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
    const listed = await deps.chats.list().catch(() => null);
    const chats = listed ?? [];
    const adopted = await adoptChats(chats, deps);
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
    if (candidates.length === 0 && listed?.length === 0) return;
    deps.schedule(() => {
        const stillClaimed = claimedSessionIds(getState());
        for (const id of candidates) if (!stillClaimed.has(id)) void deps.kill(id).catch(() => {});
        const stopUnclaimed = (kept: AcpChat[], taken: ReadonlySet<string>) => {
            for (const chat of kept) if (!taken.has(chat.agentId) && !chatClaimed(chat.agentId)) void deps.chats.stop(chat.agentId).catch(() => {});
        };
        if (listed) stopUnclaimed(listed, adopted);
        else
            void deps.chats
                .list()
                .then(async (kept) => stopUnclaimed(kept, await adoptChats(kept, deps)))
                .catch(() => {});
    }, UNCLAIMED_GRACE_MS);
}

/** Shows the chats other devices or agents started, or a board runs, and answers which they are. */
async function adoptChats(chats: AcpChat[], deps: CoreSessionRestoreDeps): Promise<Set<string>> {
    if (chats.some((chat) => chat.startedBy === null)) await deps.boards().catch(() => {});
    const boards = getState().agentBoards;
    const adopted = chats.filter((chat) => chat.startedBy !== null || runOfAgent(boards, chat.agentId));
    for (const chat of adopted) cmd.adoptChat(chat);
    return new Set(adopted.map((chat) => chat.agentId));
}
