import { useEffect, useRef } from "react";
import { agentApi } from "../api/agents";
import { getIpcTransport } from "../api/transport";
import { fetchResource } from "../state/resources";
import { agentSessionsR } from "../state/resources.defs";
import { getState, useStore } from "../state/store";
import { activeAgentId, agentIdsOf } from "../state/selectors";
import type { AgentType } from "../state/types";
import * as cmd from "../state/commands";
import { swallow } from "../state/toast";

interface AgentSyncGroup {
    type: AgentType;
    cwd: string;
    configPath?: string;
}

interface AgentSessionsChanged {
    agent: AgentType;
    cwd: string;
    configPath?: string;
}

interface AgentStateChanged {
    agentId: string;
    state: "unknown" | "working" | "blocked" | "idle" | "stopped";
    sequence: number;
    source: "screen" | "activity" | "process" | "fallback";
    confidence: "high" | "medium" | "low";
    reason: string;
    matchedRule?: string;
}

interface AgentWatchRecord {
    group: AgentSyncGroup;
    signature: string;
    watchId: number | null;
    cancelled: boolean;
    titleRetries: number;
}

const TITLE_RETRY_MS = 1_500;
const TITLE_RETRY_LIMIT = 20;

function groupKey(type: AgentType, cwd: string, configPath?: string): string {
    return `${type}\0${cwd}\0${configPath ?? ""}`;
}

/* One group's own membership, so an agent appearing in one project does not
   make every other project re-read its sessions from disk. */
interface DesiredGroup {
    group: AgentSyncGroup;
    signature: string;
}

function collectAgentSyncGroups(): Map<string, DesiredGroup> {
    const st = getState();
    const groups = new Map<string, DesiredGroup>();

    for (const sessionId of st.sessionOrder) {
        const session = st.sessions[sessionId];
        if (session?.kind !== "project" || !session.cwd) continue;
        for (const agentId of agentIdsOf(st, sessionId)) {
            const agent = st.agents[agentId];
            if (!agent || agent.launchState === "dormant") continue;
            const cwd = agent.cwd || session.cwd;
            const configPath = agent.profileId
                ? st.providerProfiles.find((profile) => profile.id === agent.profileId && profile.provider === agent.type)?.configPath
                : undefined;
            const key = groupKey(agent.type, cwd, configPath);
            const existing = groups.get(key);
            const member = `${agent.id}:${agent.resumeId ?? ""}:${agent.createdAt ?? 0}`;
            if (existing) existing.signature = `${existing.signature}|${member}`;
            else groups.set(key, { group: { type: agent.type, cwd, configPath }, signature: member });
        }
    }

    return groups;
}

function syncGroup({ type, cwd, configPath }: AgentSyncGroup): void {
    void fetchResource(agentSessionsR, type, cwd, configPath)
        .then((rows) => cmd.reconcileAgentSessions(type, cwd, configPath, rows))
        .catch(swallow("agent sessions"));
}

function groupNeedsMetadata({ type, cwd, configPath }: AgentSyncGroup): boolean {
    const state = getState();
    return state.sessionOrder.some((sessionId) => {
        const session = state.sessions[sessionId];
        if (session?.kind !== "project") return false;
        return agentIdsOf(state, sessionId).some((agentId) => {
            const agent = state.agents[agentId];
            const agentConfigPath = agent?.profileId
                ? state.providerProfiles.find((profile) => profile.id === agent.profileId && profile.provider === agent.type)?.configPath
                : undefined;
            /* Once a turn starts, ACP says which conversation the agent
               adopted, and the watcher brings the title over when the CLI
               writes it. Only an agent that has not said yet is worth asking
               about on a timer. */
            return agent?.type === type && (agent.cwd || session.cwd) === cwd && agentConfigPath === configPath && !agent.resumeId;
        });
    });
}

function useAgentSyncKey(): string {
    return useStore((s) => {
        const parts: string[] = [];
        for (const sessionId of s.sessionOrder) {
            const session = s.sessions[sessionId];
            if (session?.kind !== "project" || !session.cwd) continue;
            for (const agentId of agentIdsOf(s, sessionId)) {
                const agent = s.agents[agentId];
                if (!agent || agent.launchState === "dormant") continue;
                const configPath = agent.profileId
                    ? s.providerProfiles.find((profile) => profile.id === agent.profileId && profile.provider === agent.type)?.configPath
                    : undefined;
                parts.push(
                    `${agent.id}:${agent.type}:${agent.cwd || session.cwd}:${configPath ?? ""}:${agent.resumeId ?? ""}:${agent.createdAt ?? 0}`,
                );
            }
        }
        return parts.sort().join("|");
    });
}

export function AgentSessionSync() {
    const syncKey = useAgentSyncKey();
    const watchesRef = useRef(new Map<string, AgentWatchRecord>());
    const visibleAgentId = useStore((s) => activeAgentId(s, s.sessions[s.activeSessionId]));

    useEffect(() => {
        const controller = new AbortController();
        void getIpcTransport()
            .subscribe<AgentStateChanged>(
                "agent_state_changed",
                (event) => {
                    cmd.noteAgentActivity(event.payload.agentId, event.payload);
                },
                { signal: controller.signal },
            )
            .catch((error: unknown) => {
                if (!controller.signal.aborted) swallow("agent state listener")(error);
            });
        return () => controller.abort();
    }, []);

    useEffect(() => {
        if (visibleAgentId) cmd.clearAgentUnread(visibleAgentId);
    }, [visibleAgentId]);

    useEffect(() => {
        const controller = new AbortController();
        const watches = watchesRef.current;
        void getIpcTransport()
            .subscribe<AgentSessionsChanged>(
                "agent_sessions_changed",
                (event) => {
                    const { agent, cwd, configPath } = event.payload;
                    const record = watches.get(groupKey(agent, cwd, configPath));
                    if (record) syncGroup(record.group);
                },
                { signal: controller.signal },
            )
            .catch((error: unknown) => {
                if (!controller.signal.aborted) swallow("agent sessions listener")(error);
            });

        const titleRetryTimer = window.setInterval(() => {
            for (const record of watches.values()) {
                if (record.titleRetries >= TITLE_RETRY_LIMIT || !groupNeedsMetadata(record.group)) continue;
                record.titleRetries += 1;
                syncGroup(record.group);
            }
        }, TITLE_RETRY_MS);

        return () => {
            controller.abort();
            window.clearInterval(titleRetryTimer);
            for (const record of watches.values()) {
                record.cancelled = true;
                if (record.watchId !== null) void agentApi.watchStop(record.watchId).catch(swallow("agent sessions watch stop"));
            }
            watches.clear();
        };
    }, []);

    useEffect(() => {
        const desired = collectAgentSyncGroups();

        for (const [key, record] of watchesRef.current) {
            if (desired.has(key)) continue;
            record.cancelled = true;
            watchesRef.current.delete(key);
            if (record.watchId !== null) void agentApi.watchStop(record.watchId).catch(swallow("agent sessions watch stop"));
        }

        for (const [key, { group, signature }] of desired) {
            const existing = watchesRef.current.get(key);
            if (existing) {
                if (existing.signature === signature) continue;
                existing.signature = signature;
                existing.titleRetries = 0;
                syncGroup(existing.group);
                continue;
            }
            const record: AgentWatchRecord = { group, signature, watchId: null, cancelled: false, titleRetries: 0 };
            watchesRef.current.set(key, record);
            syncGroup(group);
            void agentApi
                .watchStart(group.type, group.cwd, group.configPath)
                .then((id) => {
                    if (record.cancelled || watchesRef.current.get(key) !== record) {
                        void agentApi.watchStop(id).catch(swallow("agent sessions watch stop"));
                    } else {
                        record.watchId = id;
                    }
                })
                .catch(swallow("agent sessions watch"));
        }
    }, [syncKey]);

    return null;
}
