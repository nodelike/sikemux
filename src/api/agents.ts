import { invokeCommand as invoke } from "./invoke";
import type { AgentRuntimeProfile } from "../agentProfiles";
import type { AgentEffort, AgentType } from "../state/types";

export interface AgentInfo {
    type: AgentType;
    label: string;
    command: string;
    available?: boolean;
    error?: string | null;
    warning?: string | null;
    profileId?: string | null;
    configPath?: string | null;
    /** Effective model inherited from the CLI's own user configuration. */
    defaultModel: string | null;
    /** Effective reasoning effort inherited from the CLI's own user configuration. */
    defaultEffort: AgentEffort | null;
}

export interface AgentModelInfo {
    /** Full identifier accepted by the CLI's --model flag. */
    id: string;
    label: string;
}

export interface AgentSession {
    id: string;
    title: string;
    mtime: number; // unix seconds
}

export interface AgentUsageWindow {
    label: string;
    usedPercent: number;
    /** Codex reports unix seconds; Claude reports an ISO-8601 timestamp. */
    resetsAt: number | string | null;
    windowMinutes: number | null;
}

export interface AgentUsage {
    provider: AgentType;
    plan: string | null;
    windows: AgentUsageWindow[];
    unavailableReason?: string | null;
}

/** A provider-scoped history result. Errors intentionally stay opaque to UI code. */
export type AgentSessionProviderResult =
    { provider: AgentInfo; status: "success"; sessions: AgentSession[] } | { provider: AgentInfo; status: "error"; sessions: [] };

const inflight = new Map<string, Promise<AgentSession[]>>();
const HISTORY_TIMEOUT_MS = 8_000;

function key(agent: string, cwd: string, configPath?: string) {
    return `${agent}\0${cwd}\0${configPath ?? ""}`;
}

async function fetchAvailable(profiles: AgentRuntimeProfile[] = []): Promise<AgentInfo[]> {
    return invoke<AgentInfo[]>("available_agents", { profiles });
}

async function fetchSessions(agent: string, cwd: string, configPath?: string): Promise<AgentSession[]> {
    const k = key(agent, cwd, configPath);
    const existing = inflight.get(k);
    if (existing) return existing;
    const p = invoke<AgentSession[]>("agent_sessions", { agent, cwd, configPath }).finally(() => {
        inflight.delete(k);
    });
    inflight.set(k, p);
    return p;
}

async function fetchSessionResults(providers: readonly AgentInfo[], cwd: string): Promise<AgentSessionProviderResult[]> {
    return Promise.all(
        providers.map(async (provider): Promise<AgentSessionProviderResult> => {
            try {
                let timer: ReturnType<typeof setTimeout> | undefined;
                const timeout = new Promise<never>((_resolve, reject) => {
                    timer = setTimeout(() => reject(new Error("agent history timed out")), HISTORY_TIMEOUT_MS);
                });
                const sessions = await Promise.race([fetchSessions(provider.type, cwd, provider.configPath ?? undefined), timeout]).finally(() => {
                    if (timer) clearTimeout(timer);
                });
                return { provider, status: "success", sessions };
            } catch {
                return { provider, status: "error", sessions: [] };
            }
        }),
    );
}

export const agentApi = {
    available: fetchAvailable,
    models: (agent: AgentType, executablePath?: string, configPath?: string): Promise<AgentModelInfo[]> =>
        invoke<AgentModelInfo[]>("agent_models", { agent, executablePath, configPath }),
    usage: (agent: AgentType, executablePath?: string, configPath?: string): Promise<AgentUsage> =>
        invoke<AgentUsage>("agent_usage", { agent, executablePath, configPath }),
    sessions: fetchSessions,
    sessionResults: fetchSessionResults,
    watchStart: (agent: AgentType, cwd: string, configPath?: string): Promise<number> =>
        invoke<number>("agent_sessions_watch_start", { agent, cwd, configPath }),
    watchStop: (id: number): Promise<void> => invoke<void>("agent_sessions_watch_stop", { id }),
};
