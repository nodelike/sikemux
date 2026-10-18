import { invokeCommand as invoke } from "./invoke";
import type { AgentType } from "../state/types";

/** A saved session the core's search found, by its title and what was said in it. */
export interface SessionMatch {
    readonly cli: AgentType;
    readonly sessionId: string;
    readonly title: string;
    /** The project folder it ran in. */
    readonly project: string;
    /** When it was last written to, in Unix milliseconds. */
    readonly activeAt: number;
    readonly snippets: readonly string[];
}

export interface SessionSearch {
    readonly query: string;
    /** Project folders to search; every project when absent. */
    readonly projects?: readonly string[];
    readonly limit?: number;
    /** How long the core may read transcripts before it answers with what it found. */
    readonly budgetMs?: number;
}

export const agentSessionsApi = {
    /** Saved sessions best matching `query`, leaving out those open now. */
    search: (search: SessionSearch) => invoke<SessionMatch[]>("agent_name_call", { method: "agentSessions", params: search }),
};
