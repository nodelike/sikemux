import { invokeCommand as invoke } from "./invoke";

/** An agent's name as the core keeps it. */
export interface AgentName {
    readonly agentId: string;
    readonly name: string;
    /** The project folder its name is unique in. */
    readonly project: string;
    /** The agent that started it, when another agent did. */
    readonly parent: string | null;
}

/** The launcher and permission mode the app runs an agent with. */
export interface AgentRun {
    readonly launcher: string;
    readonly permissionMode: string;
}

export interface NameRequest {
    readonly agentId: string;
    readonly cwd: string;
    readonly provider: string;
    /** The provider session it runs. */
    readonly resumeId?: string;
}

export const agentNamesApi = {
    /** The agent's name, given now when it has none. */
    name: (request: NameRequest) => invoke<AgentName>("agent_name_call", { method: "nameAgent", params: request }),
    rename: (agentId: string, name: string) => invoke<AgentName>("agent_name_call", { method: "renameAgent", params: { agentId, name } }),
    /**
     * Every agent the app holds, sleeping ones included, so the core keeps their names theirs and frees the names of the
     * rest, and how each runs, so an agent a terminal agent starts runs on its account and in its mode.
     */
    inApp: (agentIds: readonly string[], runs: Readonly<Record<string, AgentRun>>) =>
        invoke<null>("agent_name_call", { method: "agentsInApp", params: { agentIds, runs } }),
};
