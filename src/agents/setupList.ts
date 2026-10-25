import type { AgentInfo } from "../api/agents";
import { agentTypes, agentLabel, installCommand } from "@shared/agents/catalog";

/** Every agent Sikemux can set up: those it found, and those it can install that it did not. */
export function withMissingAgents(found: readonly AgentInfo[]): AgentInfo[] {
    const missing = agentTypes()
        .filter((type) => installCommand(type) && !found.some((agent) => agent.type === type))
        .map((type): AgentInfo => ({
            type,
            label: agentLabel(type),
            command: type,
            available: false,
            defaultModel: null,
            defaultEffort: null,
            status: { state: "missing" },
        }));
    return [...found, ...missing];
}
