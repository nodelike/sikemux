import { useCallback, useEffect, useLayoutEffect, useRef } from "react";
import { performanceTelemetry } from "../lib/performance";
import * as cmd from "../state/commands";
import { agentIdsWithLiveSessions } from "../state/agentLiveSessions";
import { getState, useStore, type StoreState } from "../state/store";
import { activeAgentId } from "../state/selectors";

export const AGENT_IDLE_SLEEP_MS = 10 * 60_000;
export const AGENT_SLEEP_POLICY_INTERVAL_MS = 30_000;
export const MAX_WARM_IDLE_AGENTS = 3;

export type HiddenAgentTimes = Map<string, number>;

function visibleAgentId(state: StoreState): string | null {
    return activeAgentId(state, state.sessions[state.activeSessionId]);
}

export function reconcileHiddenAgentTimes(state: StoreState, hiddenSince: HiddenAgentTimes, now: number): void {
    const visible = visibleAgentId(state);
    const liveIds = new Set<string>();
    for (const agent of Object.values(state.agents)) {
        if (agent.launchState === "dormant" || agent.id === visible) {
            hiddenSince.delete(agent.id);
            continue;
        }
        liveIds.add(agent.id);
        if (!hiddenSince.has(agent.id)) hiddenSince.set(agent.id, now);
    }
    for (const id of hiddenSince.keys()) {
        if (!liveIds.has(id)) hiddenSince.delete(id);
    }
}

export function agentIdsToAutoSleep(state: StoreState, hiddenSince: HiddenAgentTimes, now: number): string[] {
    const eligible = Object.values(state.agents)
        .flatMap((agent) => {
            const hiddenAt = hiddenSince.get(agent.id);
            const activity = state.agentActivity[agent.id];
            if (
                hiddenAt === undefined ||
                agent.launchState === "dormant" ||
                !agent.resumeId ||
                agent.keepAlive ||
                cmd.agentHasBackgroundWork(state, agent.id) ||
                activity?.backendState !== "idle" ||
                activity.confidence === "low"
            ) {
                return [];
            }
            return [{ id: agent.id, hiddenAt, activityAt: activity.updatedAt }];
        })
        .sort((left, right) => left.hiddenAt - right.hiddenAt || left.activityAt - right.activityAt || left.id.localeCompare(right.id));

    const sleeping = new Set(eligible.filter((agent) => now - agent.hiddenAt >= AGENT_IDLE_SLEEP_MS).map((agent) => agent.id));
    const warm = eligible.filter((agent) => !sleeping.has(agent.id));
    const overflow = Math.max(0, warm.length - MAX_WARM_IDLE_AGENTS);
    for (const agent of warm.slice(0, overflow)) sleeping.add(agent.id);
    return [...sleeping];
}

export function AgentLifecycleManager() {
    const activeSessionId = useStore((state) => state.activeSessionId);
    const sessions = useStore((state) => state.sessions);
    const agents = useStore((state) => state.agents);
    const windows = useStore((state) => state.windows);
    const agentActivity = useStore((state) => state.agentActivity);
    const backgroundWork = useStore((state) => state.agentBackgroundWork);
    const hiddenSinceRef = useRef<HiddenAgentTimes>(new Map());
    const focusedRef = useRef<string | null>(null);
    const askingRef = useRef(false);

    // Landing on a sleeping agent resumes it, so the tab the user switched to is
    // the one they get. Only the switch wakes it: sleeping the agent in front of
    // you has to stick.
    const wakeFocusedAgent = useCallback(() => {
        const state = getState();
        const focused = visibleAgentId(state);
        if (focused === focusedRef.current) return;
        focusedRef.current = focused;
        if (focused && state.agents[focused]?.launchState === "dormant") cmd.resumeAgent(focused);
    }, []);

    useLayoutEffect(wakeFocusedAgent, [activeSessionId, sessions, windows, agents, wakeFocusedAgent]);

    const enforcePolicy = useCallback(async () => {
        if (askingRef.current) return;
        const state = getState();
        const now = Date.now();
        reconcileHiddenAgentTimes(state, hiddenSinceRef.current, now);
        const sleeping = agentIdsToAutoSleep(state, hiddenSinceRef.current, now);
        if (sleeping.length === 0) return;

        askingRef.current = true;
        let live: Set<string>;
        try {
            live = await agentIdsWithLiveSessions(state, sleeping);
        } finally {
            askingRef.current = false;
        }

        /* The ask takes a moment, and the agent may have woken, been picked, or
           started a turn while it ran, so the list is worked out again. */
        const settled = agentIdsToAutoSleep(getState(), hiddenSinceRef.current, Date.now()).filter((id) => !live.has(id));
        const slept = cmd.sleepAgents(settled);
        if (slept.length > 0) performanceTelemetry.incrementCounter("agent.sleep.auto", slept.length);
        if (live.size > 0) performanceTelemetry.incrementCounter("agent.sleep.held", live.size);
    }, []);

    useEffect(() => void enforcePolicy(), [activeSessionId, sessions, windows, agents, agentActivity, backgroundWork, enforcePolicy]);

    useEffect(() => {
        const timer = window.setInterval(() => void enforcePolicy(), AGENT_SLEEP_POLICY_INTERVAL_MS);
        return () => window.clearInterval(timer);
    }, [enforcePolicy]);

    return null;
}
