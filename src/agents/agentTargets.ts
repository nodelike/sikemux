import type { AgentInfo } from "../api/agents";
import type { StoreState } from "../state/store";
import { collectPanes } from "../state/layout";
import { activeAgentId, agentIdsOf, ownerSessionId } from "../state/selectors";
import type { AgentType } from "../state/types";
import type { AgentTarget } from "./sendToAgent";

export interface AgentChoice {
    key: string;
    label: string;
    type: AgentType;
    target: AgentTarget;
}

type ChoiceState = Pick<StoreState, "agents" | "sessions" | "windows" | "windowsBySession">;

/** The project's open agents, the one in front first, then a new chat with each agent CLI that is installed. */
export function agentChoices(state: ChoiceState, sessionId: string, available: readonly AgentInfo[]): AgentChoice[] {
    const session = state.sessions[sessionId];
    if (session?.kind !== "project") return [];
    const front = activeAgentId(state, session);
    const ids = agentIdsOf(state, sessionId);
    const ordered = front && ids.includes(front) ? [front, ...ids.filter((id) => id !== front)] : ids;
    const open = ordered.flatMap((id): AgentChoice[] => {
        const agent = state.agents[id];
        return agent ? [{ key: id, label: agent.title, type: agent.type, target: { agentId: id } }] : [];
    });
    const fresh = available
        .filter((info) => info.available !== false)
        .map((info): AgentChoice => ({
            key: `new:${info.type}`,
            label: `New ${info.label} chat`,
            type: info.type,
            target: { newAgent: info.type, sessionId },
        }));
    return [...open, ...fresh];
}

/** The project in front, or else the one the person last had in front, or else the first one open. */
export function nearestProjectSessionId(state: Pick<StoreState, "sessions" | "sessionOrder" | "activeSessionId" | "lastSessionId">): string | null {
    const isProject = (id: string | null) => !!id && state.sessions[id]?.kind === "project";
    if (isProject(state.activeSessionId)) return state.activeSessionId;
    if (isProject(state.lastSessionId)) return state.lastSessionId;
    return state.sessionOrder.find(isProject) ?? null;
}

/** The open project in this folder, or else the nearest one. */
export function projectSessionForCwd(
    state: Pick<StoreState, "sessions" | "sessionOrder" | "activeSessionId" | "lastSessionId">,
    cwd: string | null,
): string | null {
    const own = cwd ? state.sessionOrder.find((id) => state.sessions[id]?.kind === "project" && state.sessions[id].cwd === cwd) : undefined;
    return own ?? nearestProjectSessionId(state);
}

/** A Markdown code block that still closes when the text itself contains backticks. */
export function codeFence(text: string, language = ""): string {
    const longest = Math.max(0, ...[...text.matchAll(/`+/g)].map((run) => run[0].length));
    const fence = "`".repeat(Math.max(3, longest + 1));
    return `${fence}${language}\n${text.replace(/\n+$/, "")}\n${fence}`;
}

/** The session whose windows hold this pane. */
export function sessionOfPane(state: Pick<StoreState, "sessionOrder" | "windowsBySession" | "windows">, paneId: string): string | null {
    const win = Object.values(state.windows).find((candidate) => collectPanes(candidate.root).some((pane) => pane.id === paneId));
    return win ? ownerSessionId(state, win.id) : null;
}
