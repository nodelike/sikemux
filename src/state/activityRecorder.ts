import { activityApi } from "../api/activity";
import { agentWindowId, ownerSessionId } from "./selectors";
import { useStore, type StoreState } from "./store";
import type { Agent } from "./types";

function agentCwd(state: StoreState, agent: Agent): string | undefined {
    if (agent.cwd) return agent.cwd;
    const windowId = agentWindowId(state, agent.id);
    const sessionId = windowId ? ownerSessionId(state, windowId) : null;
    return (sessionId && state.sessions[sessionId]?.cwd) || undefined;
}

function agentConfigPath(state: StoreState, agent: Agent): string | undefined {
    if (!agent.profileId) return undefined;
    return state.providerProfiles.find((profile) => profile.id === agent.profileId && profile.provider === agent.type)?.configPath;
}

const isWorking = (state: StoreState, id: string): boolean => state.agentActivity[id]?.backendState === "working";

/** Every stretch an agent spends working is one turn, whether it runs in a chat or a terminal. */
export function recordAgentTurns(): () => void {
    return useStore.subscribe((state, previous) => {
        if (state.agentActivity === previous.agentActivity) return;
        const ids = new Set([...Object.keys(state.agentActivity), ...Object.keys(previous.agentActivity)]);
        for (const id of ids) {
            const working = isWorking(state, id);
            if (working === isWorking(previous, id)) continue;
            const source = state.agents[id] ? state : previous;
            const agent = source.agents[id];
            const cwd = agent && agentCwd(source, agent);
            if (!agent || !cwd) continue;
            const recorded = working
                ? activityApi.turnStarted(id, cwd)
                : activityApi.turnEnded({
                      agentId: id,
                      agent: agent.type,
                      cwd,
                      sessionId: agent.resumeId,
                      configPath: agentConfigPath(source, agent),
                  });
            recorded.catch(() => {});
        }
    });
}
