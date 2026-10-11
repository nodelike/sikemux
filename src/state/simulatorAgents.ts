import { useSyncExternalStore } from "react";

/** A device an agent has attached with its simulator tools. `project` names where the agent works. */
export interface SimulatorAttachment {
    agentId: string;
    udid: string;
    name: string;
    project?: string;
}

interface SimulatorAgents {
    attachments: Readonly<Record<string, SimulatorAttachment>>;
    acting: Readonly<Record<string, true>>;
}

let current: SimulatorAgents = { attachments: {}, acting: {} };
const listeners = new Set<() => void>();

function update(next: SimulatorAgents): void {
    current = next;
    for (const listener of listeners) listener();
}

export function setSimulatorAttachments(list: readonly SimulatorAttachment[]): void {
    update({ ...current, attachments: Object.fromEntries(list.map((attachment) => [attachment.agentId, attachment])) });
}

export function noteSimulatorAttached(attachment: SimulatorAttachment): void {
    const project = attachment.project ?? current.attachments[attachment.agentId]?.project;
    update({ ...current, attachments: { ...current.attachments, [attachment.agentId]: { ...attachment, ...(project ? { project } : {}) } } });
}

export function noteSimulatorDetached(agentId: string): void {
    const { [agentId]: _gone, ...attachments } = current.attachments;
    const { [agentId]: _idle, ...acting } = current.acting;
    update({ attachments, acting });
}

export function noteSimulatorActing(agentId: string, acting: boolean): void {
    if (!!current.acting[agentId] === acting) return;
    const { [agentId]: _was, ...rest } = current.acting;
    update({ ...current, acting: acting ? { ...rest, [agentId]: true } : rest });
}

const subscribe = (listener: () => void) => {
    listeners.add(listener);
    return () => listeners.delete(listener);
};

export function useSimulatorAttachments(): Readonly<Record<string, SimulatorAttachment>> {
    return useSyncExternalStore(subscribe, () => current.attachments);
}

export function useSimulatorActing(agentId: string): boolean {
    return useSyncExternalStore(subscribe, () => !!current.acting[agentId]);
}
