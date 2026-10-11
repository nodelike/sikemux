import type { AgentStatus } from "../api/agents";

/** A few words on an agent's status for a list, or null when there is nothing to say. */
export function agentStatusLabel(status: AgentStatus | undefined): string | null {
    switch (status?.state) {
        case "missing":
            return "Not installed";
        case "broken":
            return "Not working";
        case "signedOut":
            return "Signed out";
        case "ready":
            return status.account === "apiKey" ? "Ready · API key" : status.account === "subscription" ? "Ready · subscription" : "Ready";
        default:
            return null;
    }
}

/** Whether the agent can be started at all; signed out agents can, and then ask to sign in. */
export function agentCanStart(status: AgentStatus | undefined): boolean {
    return status?.state !== "missing" && status?.state !== "broken";
}
