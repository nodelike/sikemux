import * as cmd from "../state/commands";
import { getState } from "../state/store";
import { activeAgentId } from "../state/selectors";
import type { AgentType } from "../state/types";
import { deliverToAgent, type AgentDelivery } from "./agentInbox";

/** An agent already open, or a new one of this kind started in a project. */
export type AgentTarget = { agentId: string } | { newAgent: AgentType; sessionId: string };

/**
 * Puts text and files in an agent's input and brings the agent on screen. The
 * person reads it over and sends it; nothing is sent on their behalf.
 */
export function sendToAgent(target: AgentTarget, delivery: AgentDelivery): boolean {
    if ("agentId" in target) {
        if (!getState().agents[target.agentId]) return false;
        cmd.revealAgent(target.agentId);
        deliverToAgent(target.agentId, delivery);
        return true;
    }
    if (!cmd.addAgent(target.newAgent, undefined, undefined, { sessionId: target.sessionId })) return false;
    const state = getState();
    const agentId = activeAgentId(state, state.sessions[target.sessionId]);
    if (!agentId) return false;
    if (state.activeSessionId !== target.sessionId) cmd.selectSession(target.sessionId);
    deliverToAgent(agentId, delivery);
    return true;
}
