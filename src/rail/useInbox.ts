import { useEffect } from "react";
import { INBOX_EVENT, inboxApi, type Inbox, type InboxItem } from "../api/inbox";
import { getIpcTransport } from "../api/transport";
import { externalValue, useExternalValue } from "../lib/externalValue";
import { agentWindowId, ownerSessionId, projectSessionId } from "../state/selectors";
import type { AgentShown } from "../api/generated/wire";
import { setAgentStatus } from "../state/commands/agents";
import type { StoreState } from "../state/store";
import { swallow } from "../state/toast";

interface InboxState {
    inbox: Inbox;
}

const state = externalValue<InboxState>({
    inbox: { items: [], working: 0, away: false, agentMessages: {}, remoteAgents: [], agents: [], projects: {}, rollup: null },
});
let started = false;

function take(inbox: Inbox): void {
    setAgentStatus(inbox);
    state.set({ inbox });
}

/** Follows the core's inbox for the whole app, and with it how every agent is shown. */
export function watchInbox(): void {
    if (started) return;
    started = true;
    void getIpcTransport()
        .subscribe<Inbox>(INBOX_EVENT, (event) => take(event.payload))
        .then(() => inboxApi.read())
        .then(take)
        .catch(swallow("read the inbox"));
}

/** The core's inbox, kept for the whole app once anything reads it. */
export function useInbox(): InboxState {
    useEffect(watchInbox, []);
    return useExternalValue(state);
}

/** Agents in the order the core lists them on every device; those it does not list keep their order after it. */
export function inCoreOrder<T extends { id: string }>(agents: readonly T[], shown: readonly AgentShown[]): T[] {
    const rank = (agent: T) => {
        const index = shown.findIndex((one) => one.agentId === agent.id);
        return index < 0 ? shown.length : index;
    };
    return [...agents].sort((a, b) => rank(a) - rank(b));
}

/** The project an item belongs to among those open: its agent's, else its folder's. */
export function itemSessionId(state: Pick<StoreState, "windows" | "sessionOrder" | "windowsBySession" | "sessions">, item: InboxItem): string | null {
    const windowId = item.agentId && agentWindowId(state, item.agentId);
    if (windowId) return ownerSessionId(state, windowId);
    const project = item.project ?? item.card?.project;
    return project ? projectSessionId(state, project) : null;
}
