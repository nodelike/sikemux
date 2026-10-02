import type { Agent, PtyContext, Session } from "../state/types";

export function agentCwd(agent: Agent, session: Session): string | undefined {
    return agent.cwd || session.cwd || undefined;
}

/** What a terminal agent's process is told about where it runs. */
export function agentPtyContext(agent: Agent, session: Session): PtyContext {
    const cwd = agentCwd(agent, session);
    return {
        sessionId: session.id,
        sessionName: session.name,
        sessionKind: session.kind,
        ...(session.kind === "project" && cwd ? { project: cwd } : {}),
        agentId: agent.id,
        agentType: agent.type,
    };
}
