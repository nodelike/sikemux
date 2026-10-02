import type { CoreSession } from "../api/coreSessions";
import { afterAgentExit } from "../agents/tuiRecovery";
import { collectPanes } from "./layout";
import type { StoreState } from "./store";

/** Every core terminal the workspace shows or will show, whether or not its pane is on screen. */
export function claimedSessionIds(state: Pick<StoreState, "windows" | "agents">): Set<number> {
    const claimed = new Set<number>();
    for (const window of Object.values(state.windows)) {
        for (const pane of collectPanes(window.root)) if (pane.ptyId !== undefined) claimed.add(pane.ptyId);
    }
    for (const agent of Object.values(state.agents)) if (agent.ptyId !== undefined) claimed.add(agent.ptyId);
    return claimed;
}

export interface AgentSessionPlan {
    /** Their terminal still runs, so they come back live in it. */
    readonly live: readonly string[];
    /** Their terminal is gone; they keep the resume they had. */
    readonly gone: readonly string[];
    /** Their terminal is gone and they have no session to resume, so nothing is left to show. */
    readonly dropped: readonly string[];
    /** Their terminal crashed while the app was closed; they come back on their saved conversation below its last screen. */
    readonly resume: readonly string[];
}

export function agentSessionPlan(state: Pick<StoreState, "agents">, sessions: readonly CoreSession[]): AgentSessionPlan {
    const terminals = new Map(sessions.filter((session) => session.kind === "terminal").map((session) => [session.id, session]));
    const live: string[] = [];
    const gone: string[] = [];
    const dropped: string[] = [];
    const resume: string[] = [];
    for (const agent of Object.values(state.agents)) {
        if (agent.ptyId === undefined) continue;
        const session = terminals.get(agent.ptyId);
        if (session?.running) live.push(agent.id);
        else if (session && crashed(session, agent.resumeId)) resume.push(agent.id);
        else (agent.resumeId ? gone : dropped).push(agent.id);
    }
    return { live, gone, dropped, resume };
}

function crashed(session: CoreSession, resumeId: string | undefined): boolean {
    const exit = { code: session.exit?.code ?? null, signal: session.exit?.signal ?? null, killed: session.killed };
    return afterAgentExit({ exit, resumeId, lastResumeAt: null, now: 0 }) === "resume";
}

/** Terminals this app started that nothing in the workspace refers to. Tasks are taken over separately. */
export function unclaimedTerminals(sessions: readonly CoreSession[], claimed: ReadonlySet<number>): number[] {
    return sessions
        .filter((session) => session.kind === "terminal" && session.startedBy === null && !claimed.has(session.id))
        .map((session) => session.id);
}
