import { isAgentType } from "../../agents/agentLaunch";
import type { Agent, Session } from "../types";
import { addAgent } from "./agents";
import { createProjectSession } from "./sessions";

/**
 * A link that brings this agent's conversation back up, in its project, from
 * anywhere on the Mac. Null until the agent has a conversation to come back to.
 */
export function agentLink(agent: Agent, session: Session): string | null {
    if (!agent.resumeId || session.kind !== "project") return null;
    return `sikemux://agent/${agent.type}/${encodeURIComponent(agent.resumeId)}?project=${encodeURIComponent(session.cwd)}`;
}

/** Opens what a sikemux:// link points at, and says whether it understood the link. */
export function routeDeepLink(link: string): boolean {
    let url: URL;
    try {
        url = new URL(link);
    } catch {
        return false;
    }
    if (url.protocol !== "sikemux:" || url.hostname !== "agent") return false;
    const [type, resumeId] = url.pathname.split("/").filter(Boolean).map(decodeURIComponent);
    const project = url.searchParams.get("project");
    if (!type || !isAgentType(type) || !resumeId || !project) return false;
    createProjectSession(project);
    return addAgent(type, resumeId);
}
