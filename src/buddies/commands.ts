import { create } from "zustand";
import { toAgentBuddy } from "./agentBuddy";
import { mutate } from "../state/store";
import { resolveBuddy } from "./catalogue";

export function setAgentBuddyShape(id: string, pack: string, shape: string): void {
    mutate((d) => {
        const agent = d.agents[id];
        if (agent) agent.buddy = { color: agent.buddy?.color ?? resolveBuddy(agent, { pack, shape }).colorIndex, pack, shape };
    });
}

export function setAgentBuddyColor(id: string, color: number): void {
    mutate((d) => {
        const agent = d.agents[id];
        if (agent) agent.buddy = { ...agent.buddy, color };
    });
}

/** Puts back the character an agent wore before it closed, as the core kept it. */
export function wearBuddy(id: string, kept: unknown): void {
    const buddy = toAgentBuddy(kept);
    if (!buddy) return;
    mutate((d) => {
        const agent = d.agents[id];
        if (agent) agent.buddy = buddy;
    });
}

/** Lets the agent follow the default style again, keeping its colour. */
export function resetAgentBuddy(id: string): void {
    mutate((d) => {
        const agent = d.agents[id];
        if (agent?.buddy) agent.buddy = { color: agent.buddy.color };
    });
}

export const useBuddyPicker = create<{ agentId: string | null }>(() => ({ agentId: null }));
export const openBuddyPicker = (agentId: string): void => useBuddyPicker.setState({ agentId });
export const closeBuddyPicker = (): void => useBuddyPicker.setState({ agentId: null });
