import { coreCall } from "./core";
import { onCoreChanged } from "./coreChanged";
import type { IpcUnsubscribe } from "./transport";
import type { CatalogAgent } from "@shared/agents/catalog";
import type { AgentEffort, AgentType, ProviderProfile, ProviderProfileSelection } from "../state/types";

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
    /** The worktree of the project the chat ran in, when it ran in one. */
    cwd?: string;
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

/** The person's agent accounts, as the core keeps them. */
export interface AgentAccountBook {
    profiles: ProviderProfile[];
    selected: ProviderProfileSelection;
    /** Whether a chat moves to the next signed-in account at a usage limit, by provider. */
    autoSwitch: Partial<Record<AgentType, boolean>>;
}

const inflight = new Map<string, Promise<AgentSession[]>>();

/** One of the core's calls about the person's agent accounts. */
function accountCall<T>(method: string, params: Record<string, unknown> = {}): Promise<T> {
    return coreCall<T>(method, params);
}

function key(agent: string, cwd: string, configPath?: string) {
    return `${agent}\0${cwd}\0${configPath ?? ""}`;
}

async function fetchSessions(agent: string, cwd: string, configPath?: string): Promise<AgentSession[]> {
    const k = key(agent, cwd, configPath);
    const existing = inflight.get(k);
    if (existing) return existing;
    const p = coreCall<AgentSession[]>("savedSessions", { agent, cwd, configPath }).finally(() => {
        inflight.delete(k);
    });
    inflight.set(k, p);
    return p;
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

export const agentApi = {
    /** The agents found here, each on the account chosen for it. */
    available: (): Promise<AgentInfo[]> => coreCall("installedAgents"),
    /** Those, with the agents Sikemux can install that it did not find, in the order to set them up. */
    setup: (): Promise<AgentInfo[]> => coreCall("agentSetup"),
    /** The agents the core can run, which every view names and offers them from. */
    catalog: (): Promise<CatalogAgent[]> => coreCall("agentCatalog"),
    /** The accounts the core keeps; `null` until the app first gives it some. */
    accounts: (): Promise<AgentAccountBook | null> => accountCall("agentAccounts"),
    /** Keeps the account, in place of the one with its id; each of these answers with the accounts as they are now. */
    saveAccount: (profile: ProviderProfile): Promise<AgentAccountBook> => accountCall("saveAgentAccount", { profile }),
    removeAccount: (id: string): Promise<AgentAccountBook> => accountCall("removeAgentAccount", { id }),
    /** Makes the account the one `provider` runs on, and moves its open chats there. */
    selectAccount: (provider: AgentType, id: string): Promise<AgentAccountBook> => accountCall("selectAgentAccount", { provider, id }),
    setAutoSwitch: (provider: AgentType, on: boolean): Promise<AgentAccountBook> => accountCall("setAccountAutoSwitch", { provider, on }),
    /** Has the core ask every agent its status again; until it answers, the last answers are shown. */
    refreshStatuses: (): Promise<void> => accountCall("forgetAgentStatus"),
    /** Forgets one account's sign-in and usage, after they changed. */
    forgetAccount: (agent: AgentType, configPath?: string): Promise<void> => accountCall("forgetAgentStatus", { provider: agent, configPath }),
    markSignedOut: (agent: AgentType, configPath?: string): Promise<void> => accountCall("markAgentSignedOut", { provider: agent, configPath }),
    usage: (agent: AgentType, configPath?: string): Promise<AgentUsage> => accountCall("agentUsage", { provider: agent, configPath }),
    account: (agent: AgentType, configPath?: string): Promise<AgentAccountStatus> =>
        accountCall("agentAccountStatus", { provider: agent, configPath }),
    addAccount: (agent: AgentType, name: string): Promise<string> => accountCall("addAgentAccount", { provider: agent, name }),
    /** Stops a chat a usage limit stopped from sending its prompt again once the limit resets. */
    cancelResume: (agentId: string): Promise<void> => accountCall("cancelResume", { agentId }),
    /** Sends the prompt a failed turn stopped again. */
    retryFailed: (agentId: string): Promise<void> => accountCall("retryFailed", { agentId }),
    /** Asks a chat's agent to sign in by one of the methods it handles itself. */
    authenticateChat: (agentId: string, methodId: string): Promise<void> => accountCall("authenticateChat", { agentId, methodId }),
    /** Lets go of the prompt a failed turn stopped, so nothing sends it again. */
    discardFailed: (agentId: string): Promise<void> => accountCall("discardFailed", { agentId }),
    signIn: (agent: AgentType, executablePath?: string, configPath?: string): Promise<void> =>
        coreCall<void>("agentSignIn", { agent, executablePath, configPath }),
    signInCode: (agent: AgentType, configPath: string | undefined, code: string): Promise<void> =>
        coreCall<void>("agentSignInCode", { agent, configPath, code }),
    cancelSignIn: (agent: AgentType, configPath?: string): Promise<void> => coreCall<void>("cancelAgentSignIn", { agent, configPath }),
    signOut: (agent: AgentType, executablePath?: string, configPath?: string): Promise<void> =>
        coreCall<void>("agentSignOut", { agent, executablePath, configPath }),
    /** Each page a sign-in on the host opens, once, for when the person is not at the host's browser. */
    onSignInPage: (listener: (page: AgentSignInPage) => void, signal?: AbortSignal): Promise<IpcUnsubscribe> => {
        const shown = new Set<string>();
        const read = () =>
            coreCall<AgentSignInPage[]>("agentSignIns").then(
                (pages) => {
                    for (const page of pages) {
                        if (shown.has(page.url) || signal?.aborted) continue;
                        shown.add(page.url);
                        listener(page);
                    }
                },
                () => {},
            );
        const stop = onCoreChanged("agentSignIn", () => void read());
        signal?.addEventListener("abort", stop, { once: true });
        return Promise.resolve(stop);
    },
    sessions: fetchSessions,
    recent: (request: RecentChatsRequest): Promise<RecentChatsPage> => coreCall<RecentChatsPage>("recentSessions", { request }),
    renameSession: (agent: AgentType, cwd: string, sessionId: string, title: string, executablePath?: string, configPath?: string): Promise<void> =>
        coreCall<void>("renameSession", { agent, cwd, sessionId, title, executablePath, configPath }),
    deleteSession: (agent: AgentType, cwd: string, sessionId: string, executablePath?: string, configPath?: string): Promise<void> =>
        coreCall<void>("deleteSession", { agent, cwd, sessionId, executablePath, configPath }),
    relocateSession: (agent: AgentType, fromCwd: string, toCwd: string, sessionId: string, configPath?: string): Promise<void> =>
        coreCall<void>("relocateSession", { agent, fromCwd, toCwd, sessionId, configPath }),
};
