import { create } from "zustand";
import { getState, mutate } from "../state/store";
import { leastUsedColor } from "./catalogue";

/** Gives every agent that has no colour yet the one its neighbours wear least, oldest first. */
export function assignBuddyColors(): void {
    const waiting = Object.values(getState().agents)
        .filter((agent) => !agent.buddy)
        .sort((a, b) => (a.createdAt ?? 0) - (b.createdAt ?? 0));
    if (waiting.length === 0) return;
    mutate((d) => {
        for (const { id } of waiting) {
            const agent = d.agents[id];
            if (agent && !agent.buddy) agent.buddy = { color: leastUsedColor(Object.values(d.agents), agent.cwd) };
        }
    });
}

export function setAgentBuddyShape(id: string, pack: string, shape: string): void {
    mutate((d) => {
        const agent = d.agents[id];
        if (agent) agent.buddy = { color: agent.buddy?.color ?? leastUsedColor(Object.values(d.agents), agent.cwd), pack, shape };
    });
}

export function setAgentBuddyColor(id: string, color: number): void {
    mutate((d) => {
        const agent = d.agents[id];
        if (agent) agent.buddy = { ...agent.buddy, color };
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
