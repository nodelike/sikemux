import type { AgentInfo } from "../api/agents";
import { agentSupportsChat } from "../agents/agentLaunch";
import { selectedAgentRuntimeProfiles } from "../agents/agentProfiles";
import { sendToAgent } from "../agents/sendToAgent";
import { fetchResource, peekResource } from "../state/resources";
import { agentCatalogR } from "../state/resources.defs";
import { getState, type StoreState } from "../state/store";
import { notify } from "../state/toast";
import type { AgentType } from "../state/types";
import { failureMessage } from "./api";
import { loadTrackedContext } from "./tracked";
import type { RepoRef } from "./types";

/** The agent launched last when it can chat, or else the first installed one that can. */
export function chatAgentFor(catalog: readonly AgentInfo[], last: AgentType | null): AgentType | null {
    const chatting = catalog.filter((agent) => agent.available !== false && agentSupportsChat(agent.type)).map((agent) => agent.type);
    return chatting.find((type) => type === last) ?? chatting[0] ?? null;
}

/** The open project in this folder. */
export function projectSessionIn(state: Pick<StoreState, "sessions" | "sessionOrder">, cwd: string): string | null {
    return state.sessionOrder.find((id) => state.sessions[id]?.kind === "project" && state.sessions[id].cwd === cwd) ?? null;
}

/**
 * Starts a chat in the issue's project with the issue already in its input.
 * The person reads it over, says what they want, and sends it themselves.
 */
export async function workOnIssue(repo: RepoRef, number: number, cwd: string): Promise<void> {
    const state = getState();
    const sessionId = projectSessionIn(state, cwd);
    if (!sessionId) {
        notify("error", "Open the project to start an agent in it");
        return;
    }
    const runtime = selectedAgentRuntimeProfiles(state.providerProfiles, state.selectedProviderProfileIds);
    const catalog = peekResource(agentCatalogR, runtime) ?? (await fetchResource(agentCatalogR, runtime).catch((): AgentInfo[] => []));
    const type = chatAgentFor(catalog, state.lastAgentType);
    if (!type) {
        notify("error", "No agent that can chat is installed");
        return;
    }
    let context;
    try {
        context = await loadTrackedContext(repo, "issue", number);
    } catch (failure) {
        notify("error", `Could not read #${number}: ${failureMessage(failure)}`);
        return;
    }
    if (!sendToAgent({ newAgent: type, sessionId }, { context: [context] })) notify("error", "Could not start an agent in this project");
}
