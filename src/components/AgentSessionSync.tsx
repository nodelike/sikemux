import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { agentApi } from "../api/agents";
import { fetchResource } from "../state/resources";
import { agentSessionsR } from "../state/resources.defs";
import { getState, useStore } from "../state/store";
import type { AgentType } from "../state/types";
import * as cmd from "../state/commands";
import { swallow } from "../state/toast";

interface AgentSyncGroup {
    type: AgentType;
    cwd: string;
}

interface AgentSessionsChanged {
    agent: AgentType;
    cwd: string;
}

interface AgentStateChanged {
    agentId: string;
    state: "unknown" | "working" | "blocked" | "idle";
    sequence: number;
    source: "screen" | "activity" | "process" | "fallback";
    confidence: "high" | "medium" | "low";
    reason: string;
    matchedRule?: string;
}

function groupKey(type: AgentType, cwd: string): string {
    return `${type}\0${cwd}`;
}

function collectAgentSyncGroups(): AgentSyncGroup[] {
    const st = getState();
    const groups = new Map<string, AgentSyncGroup>();

    for (const sessionId of st.sessionOrder) {
        const session = st.sessions[sessionId];
        if (session?.kind !== "project" || !session.cwd) continue;
        for (const agentId of st.agentsBySession[sessionId] ?? []) {
            const agent = st.agents[agentId];
            if (!agent) continue;
            groups.set(groupKey(agent.type, session.cwd), { type: agent.type, cwd: session.cwd });
        }
    }

    return [...groups.values()];
}

function useAgentSyncKey(): string {
    return useStore((s) => {
        const parts: string[] = [];
        for (const sessionId of s.sessionOrder) {
            const session = s.sessions[sessionId];
            if (session?.kind !== "project" || !session.cwd) continue;
            for (const agentId of s.agentsBySession[sessionId] ?? []) {
                const agent = s.agents[agentId];
                if (!agent) continue;
                parts.push(`${agent.id}:${agent.type}:${session.cwd}:${agent.resumeId ?? ""}:${agent.createdAt ?? 0}`);
            }
        }
        return parts.sort().join("|");
    });
}

export function AgentSessionSync() {
    const syncKey = useAgentSyncKey();
    const visibleAgentId = useStore((s) => {
        const session = s.sessions[s.activeSessionId];
        return session?.view === "agent" ? session.activeAgentId : null;
    });

    useEffect(() => {
        const unlisten = listen<AgentStateChanged>("agent_state_changed", (event) => {
            cmd.noteAgentActivity(event.payload.agentId, event.payload);
        });
        return () => void unlisten.then((off) => off());
    }, []);

    useEffect(() => {
        if (visibleAgentId) cmd.clearAgentUnread(visibleAgentId);
    }, [visibleAgentId]);

    useEffect(() => {
        if (!syncKey) return;

        let cancelled = false;
        const watchIds: number[] = [];
        const groups = collectAgentSyncGroups();
        const activeGroups = new Set(groups.map((group) => groupKey(group.type, group.cwd)));

        const syncGroup = (type: AgentType, cwd: string) => {
            void fetchResource(agentSessionsR, type, cwd)
                .then((rows) => cmd.reconcileAgentSessions(type, cwd, rows))
                .catch(swallow("agent sessions"));
        };

        for (const group of groups) {
            syncGroup(group.type, group.cwd);
            void agentApi
                .watchStart(group.type, group.cwd)
                .then((id) => {
                    if (cancelled) {
                        void agentApi.watchStop(id).catch(swallow("agent sessions watch stop"));
                    } else {
                        watchIds.push(id);
                    }
                })
                .catch(swallow("agent sessions watch"));
        }

        const unlisten = listen<AgentSessionsChanged>("agent_sessions_changed", (event) => {
            const { agent, cwd } = event.payload;
            if (!activeGroups.has(groupKey(agent, cwd))) return;
            syncGroup(agent, cwd);
        });

        return () => {
            cancelled = true;
            void unlisten.then((off) => off());
            for (const id of watchIds) {
                void agentApi.watchStop(id).catch(swallow("agent sessions watch stop"));
            }
        };
    }, [syncKey]);

    return null;
}
