import { invokeCommand as invoke } from "./invoke";
import { getIpcTransport, type IpcUnsubscribe } from "./transport";
import type { AgentRuntimeProfile } from "../agents/agentProfiles";
import type { AgentEffort, AgentType } from "../state/types";

/** Whether an agent can be used right now; `unknown` when its CLI has no way to say if it is signed in. */
export type AgentStatus =
    | { state: "missing" }
    | { state: "broken"; reason: string }
    | { state: "signedOut" }
    | { state: "ready"; account: string | null }
    | { state: "unknown" };

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
    status?: AgentStatus;
}

export interface AgentSession {
    id: string;
    title: string;
    mtime: number; // unix seconds
}

/** Where the last page of saved chats ended; handed back to fetch the next one. */
export interface RecentCursor {
    atMs: number;
    agent: string;
    key: string;
}

export interface RecentChatsRequest {
    providers: { agent: AgentType; configPath?: string | null }[];
    projects: string[];
    limit: number;
    cursor?: RecentCursor | null;
    query?: string;
    /** Chats already open as agents, left out so a page stays full. */
    exclude: { agent: AgentType; id: string }[];
}

export interface RecentChat extends AgentSession {
    agent: AgentType;
    /** The project folder the chat ran in. */
    project: string;
}

export interface RecentChatsPage {
    sessions: RecentChat[];
    next: RecentCursor | null;
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

const inflight = new Map<string, Promise<AgentSession[]>>();

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

/** One session Claude Code is running now, as it reports itself. */
export interface LiveAgentSession {
    sessionId: string;
    /** `busy`, `waiting`, `shell`, or `idle` — anything but `idle` still has something going. */
    status: string;
}

/** Who one account is signed in as, in the agent's own words. */
export interface AgentAccountStatus {
    signedIn: boolean;
    /** The person's own name on the account, where the agent keeps one. */
    name: string | null;
    email: string | null;
    plan: string | null;
    organization: string | null;
    /** `subscription`, `apiKey`, or whatever else the CLI signs in with. */
    method: string | null;
    /** Where the account keeps its chats: accounts that share it can take over each other's chats. */
    sessions: string | null;
}

/** The page a running sign-in opened, for when the browser did not. */
export interface AgentSignInPage {
    agent: AgentType;
    configPath: string | null;
    url: string;
}

/** How full a saved session's context window was. Claude does not record the window's size. */
export interface SavedSessionContext {
    used: number;
    size: number | null;
}

export const agentApi = {
    available: fetchAvailable,
    /** Forgets every agent's status, so the next look asks each one again. */
    refreshStatuses: (): Promise<void> => invoke<void>("refresh_agent_statuses"),
    markSignedOut: (agent: AgentType, configPath?: string): Promise<void> => invoke<void>("mark_agent_signed_out", { agent, configPath }),
    usage: (agent: AgentType, executablePath?: string, configPath?: string): Promise<AgentUsage> =>
        invoke<AgentUsage>("agent_usage", { agent, executablePath, configPath }),
    account: (agent: AgentType, executablePath?: string, configPath?: string): Promise<AgentAccountStatus> =>
        invoke<AgentAccountStatus>("agent_account_status", { agent, executablePath, configPath }),
    addAccount: (agent: AgentType, name: string): Promise<string> => invoke<string>("agent_account_add", { agent, name }),
    signIn: (agent: AgentType, executablePath?: string, configPath?: string): Promise<void> =>
        invoke<void>("agent_account_sign_in", { agent, executablePath, configPath }),
    signInCode: (agent: AgentType, configPath: string | undefined, code: string): Promise<void> =>
        invoke<void>("agent_account_sign_in_code", { agent, configPath, code }),
    cancelSignIn: (agent: AgentType, configPath?: string): Promise<void> => invoke<void>("agent_account_sign_in_cancel", { agent, configPath }),
    signOut: (agent: AgentType, executablePath?: string, configPath?: string): Promise<void> =>
        invoke<void>("agent_account_sign_out", { agent, executablePath, configPath }),
    onSignInPage: (listener: (page: AgentSignInPage) => void, signal?: AbortSignal): Promise<IpcUnsubscribe> =>
        getIpcTransport().subscribe<AgentSignInPage>("agent_account_sign_in", (event) => listener(event.payload), { signal }),
    sessions: fetchSessions,
    recent: (request: RecentChatsRequest): Promise<RecentChatsPage> => invoke<RecentChatsPage>("agent_recent_sessions", { request }),
    sessionContext: (agent: AgentType, cwd: string, sessionId: string, configPath?: string): Promise<SavedSessionContext | null> =>
        invoke<SavedSessionContext | null>("agent_session_context", { agent, cwd, sessionId, configPath }),
    renameSession: (agent: AgentType, cwd: string, sessionId: string, title: string, executablePath?: string, configPath?: string): Promise<void> =>
        invoke<void>("agent_session_rename", { agent, cwd, sessionId, title, executablePath, configPath }),
    deleteSession: (agent: AgentType, cwd: string, sessionId: string, executablePath?: string, configPath?: string): Promise<void> =>
        invoke<void>("agent_session_delete", { agent, cwd, sessionId, executablePath, configPath }),
    watchStart: (agent: AgentType, cwd: string, configPath?: string): Promise<number> =>
        invoke<number>("agent_sessions_watch_start", { agent, cwd, configPath }),
    watchStop: (id: number): Promise<void> => invoke<void>("agent_sessions_watch_stop", { id }),
    liveSessions: (configPath?: string): Promise<LiveAgentSession[]> => invoke<LiveAgentSession[]>("live_agent_sessions", { configPath }),
};
