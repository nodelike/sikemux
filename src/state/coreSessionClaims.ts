import type { SessionInfo } from "@shared/wire.generated";
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

/** The terminal agents whose terminal still runs in the core. */
export function runningTerminalAgents(state: Pick<StoreState, "agents">, sessions: readonly SessionInfo[]): string[] {
    const running = new Set(sessions.filter((session) => session.kind === "terminal" && session.running).map((session) => session.id));
    return Object.values(state.agents)
        .filter((agent) => agent.ptyId !== undefined && running.has(agent.ptyId))
        .map((agent) => agent.id);
}

/** Terminals this app started that nothing in the workspace refers to. Tasks are taken over separately. */
export function unclaimedTerminals(sessions: readonly SessionInfo[], claimed: ReadonlySet<number>): number[] {
    return sessions
        .filter((session) => session.kind === "terminal" && session.startedBy === null && !claimed.has(session.id))
        .map((session) => session.id);
}
