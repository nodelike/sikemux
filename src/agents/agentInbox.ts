import type { PromptContext } from "../api/acp";

/** Text, files and context items handed to an agent from elsewhere in the app, for the person to send. */
export interface AgentDelivery {
    text?: string;
    paths?: string[];
    context?: PromptContext[];
}

type Receiver = (delivery: AgentDelivery) => void;

const receivers = new Map<string, Receiver>();
const pending = new Map<string, AgentDelivery[]>();

/**
 * Called by whichever input the agent is showing, its chat composer or its
 * terminal. A delivery made while neither is on screen waits here and lands
 * as soon as one is.
 */
export function receiveForAgent(agentId: string, receiver: Receiver): () => void {
    receivers.set(agentId, receiver);
    const waiting = pending.get(agentId);
    if (waiting) {
        pending.delete(agentId);
        for (const delivery of waiting) receiver(delivery);
    }
    return () => {
        if (receivers.get(agentId) === receiver) receivers.delete(agentId);
    };
}

export function deliverToAgent(agentId: string, delivery: AgentDelivery): void {
    const receiver = receivers.get(agentId);
    if (receiver) {
        receiver(delivery);
        return;
    }
    pending.set(agentId, [...(pending.get(agentId) ?? []), delivery]);
}

export function forgetAgentDeliveries(agentId: string): void {
    pending.delete(agentId);
}
