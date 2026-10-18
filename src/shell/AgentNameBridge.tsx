import { useEffect, useMemo, useRef, useState } from "react";
import { agentCwd } from "../agents/agentPtyContext";
import { effectiveMode } from "../agents/agentLaunch";
import { agentNamesApi, type AgentRun, type NameRequest } from "../api/agentNames";
import { launcherId } from "../remote/workspace";
import * as cmd from "../state/commands";
import { agentIdsOf } from "../state/selectors";
import { useStore, type StoreState } from "../state/store";
import { swallow } from "../state/toast";

const RETRY_MS = 3_000;

function nameRequests(state: Pick<StoreState, "agents" | "windows" | "sessions" | "sessionOrder" | "windowsBySession">): NameRequest[] {
    return state.sessionOrder.flatMap((sessionId) => {
        const session = state.sessions[sessionId];
        if (!session) return [];
        return agentIdsOf(state, sessionId).flatMap((agentId) => {
            const agent = state.agents[agentId];
            const cwd = agent && agentCwd(agent, session);
            if (!agent || !cwd) return [];
            return [{ agentId, cwd, provider: agent.type, ...(agent.resumeId ? { resumeId: agent.resumeId } : {}) }];
        });
    });
}

function heldAgents(agents: StoreState["agents"]): { agentIds: string[]; runs: Record<string, AgentRun> } {
    const agentIds = Object.keys(agents).sort();
    const runs: Record<string, AgentRun> = {};
    for (const id of agentIds) {
        const agent = agents[id];
        runs[id] = { launcher: launcherId(agent.type, agent.profileId), permissionMode: effectiveMode(agent) };
    }
    return { agentIds, runs };
}

/**
 * Tells the core every agent the app holds, so a closed one's name is free again, then asks for each one's name. The
 * core names an agent as it starts; this also names the ones restored from before and those opened again from Recent,
 * which get a new name, and tells the core which provider session each runs.
 */
export function AgentNameBridge() {
    const agents = useStore((s) => s.agents);
    const windows = useStore((s) => s.windows);
    const sessions = useStore((s) => s.sessions);
    const sessionOrder = useStore((s) => s.sessionOrder);
    const windowsBySession = useStore((s) => s.windowsBySession);
    const held = useMemo(() => JSON.stringify(heldAgents(agents)), [agents]);
    const requests = useMemo(
        () => JSON.stringify(nameRequests({ agents, windows, sessions, sessionOrder, windowsBySession })),
        [agents, windows, sessions, sessionOrder, windowsBySession],
    );
    const told = useRef<Promise<unknown>>(Promise.resolve());
    const asked = useRef(new Map<string, string>());
    const [retries, setRetries] = useState(0);

    useEffect(() => {
        const { agentIds, runs } = JSON.parse(held) as ReturnType<typeof heldAgents>;
        told.current = agentNamesApi.inApp(agentIds, runs).catch(swallow("tell the core which agents the app holds"));
    }, [held]);

    useEffect(() => {
        let retry: number | undefined;
        for (const request of JSON.parse(requests) as NameRequest[]) {
            const key = JSON.stringify(request);
            if (asked.current.get(request.agentId) === key) continue;
            asked.current.set(request.agentId, key);
            told.current
                .then(() => agentNamesApi.name(request))
                .then(
                    (named) => cmd.setAgentName(request.agentId, named.name, named.parent),
                    (error: unknown) => {
                        asked.current.delete(request.agentId);
                        swallow("name an agent")(error);
                        window.clearTimeout(retry);
                        retry = window.setTimeout(() => setRetries((count) => count + 1), RETRY_MS);
                    },
                );
        }
        return () => window.clearTimeout(retry);
    }, [requests, retries]);

    return null;
}
