import { useMemo } from "react";
import { agentLabel } from "../agents/catalog";
import type { PromptContext } from "../api/acp";
import { MENTION_URI } from "./promptContext";
import { agentDisplay, type AgentDisplay } from "../state/agentStatus";
import { agentIdsOf, agentWindowId, ownerSessionId } from "../state/selectors";
import { useStore } from "../state/store";
import type { Agent, AgentType } from "../state/types";

/** Another agent, as the composer offers it after `@`. */
export interface Mate {
    id: string;
    name: string;
    /** The project it works in, when that is not this agent's own. */
    project: string | null;
    type: AgentType;
    title: string;
    buddy: Agent["buddy"];
    /** How it is shown beside its name; none while it sleeps. */
    display: AgentDisplay | null;
}

/** The other named agents: this project's first, then every other project's, each in strip order. */
export function useMates(agentId: string): Mate[] {
    const agents = useStore((s) => s.agents);
    const sessions = useStore((s) => s.sessions);
    const windows = useStore((s) => s.windows);
    const windowsBySession = useStore((s) => s.windowsBySession);
    const sessionOrder = useStore((s) => s.sessionOrder);
    const activity = useStore((s) => s.agentActivity);
    return useMemo(() => {
        const windowId = agentWindowId({ windows }, agentId);
        const sessionId = windowId ? ownerSessionId({ sessionOrder, windowsBySession }, windowId) : null;
        const order = sessionId ? [sessionId, ...sessionOrder.filter((id) => id !== sessionId)] : sessionOrder;
        return order.flatMap((sid) => {
            const project = sid === sessionId ? null : (sessions[sid]?.name ?? null);
            return agentIdsOf({ windows, windowsBySession }, sid).flatMap((id) => {
                const agent = agents[id];
                if (!agent?.name || id === agentId) return [];
                const display = agent.launchState === "dormant" ? null : agentDisplay(activity[id]?.state);
                return [{ id, name: agent.name, project, type: agent.type, title: agent.title, buddy: agent.buddy, display }];
            });
        });
    }, [agentId, agents, sessions, windows, windowsBySession, sessionOrder, activity]);
}

export function mateNote(mate: Mate, status: string, earlier?: string): string {
    return [
        mate.project
            ? `@${mate.name} is an agent in another project, ${mate.project}, not the person.`
            : `@${mate.name} is another agent in this project, not the person.`,
        ...(earlier ? [`Sikemux opened its earlier chat "${earlier}" again for this message, under this new name.`] : []),
        `Agent id: ${mate.id}`,
        `CLI: ${agentLabel(mate.type)}`,
        `Status: ${status}`,
        `Working on: ${mate.title}`,
        `To message it, call agent_send with to "${mate.project ? mate.id : mate.name}".`,
    ].join("\n");
}

/** What the agent is told of each agent the message names with `@`, so it knows who is meant. */
export function mentionContext(text: string, mates: readonly Mate[]): PromptContext[] {
    const named = new Set([...text.matchAll(/(?:^|\s)@([\p{L}][\p{L}\p{N}]*)/gu)].map((match) => match[1].toLowerCase()));
    const seen = new Set<string>();
    return mates
        .filter((mate) => {
            const name = mate.name.toLowerCase();
            if (!named.has(name) || seen.has(name)) return false;
            seen.add(name);
            return true;
        })
        .map((mate) => ({ uri: `${MENTION_URI}${mate.id}`, title: `@${mate.name}`, text: mateNote(mate, mate.display?.label ?? "Asleep") }));
}
