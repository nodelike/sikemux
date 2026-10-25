import { externalValue, useExternalValue } from "@shared/lib/externalValue";

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

const agents = externalValue<SimulatorAgents>({ attachments: {}, acting: {} });

export function setSimulatorAttachments(list: readonly SimulatorAttachment[]): void {
    agents.set({ ...agents.get(), attachments: Object.fromEntries(list.map((attachment) => [attachment.agentId, attachment])) });
}

export function noteSimulatorAttached(attachment: SimulatorAttachment): void {
    const current = agents.get();
    const project = attachment.project ?? current.attachments[attachment.agentId]?.project;
    agents.set({ ...current, attachments: { ...current.attachments, [attachment.agentId]: { ...attachment, ...(project ? { project } : {}) } } });
}

export function noteSimulatorDetached(agentId: string): void {
    const current = agents.get();
    const { [agentId]: _gone, ...attachments } = current.attachments;
    const { [agentId]: _idle, ...acting } = current.acting;
    agents.set({ attachments, acting });
}

export function noteSimulatorActing(agentId: string, acting: boolean): void {
    const current = agents.get();
    if (!!current.acting[agentId] === acting) return;
    const { [agentId]: _was, ...rest } = current.acting;
    agents.set({ ...current, acting: acting ? { ...rest, [agentId]: true } : rest });
}

export function useSimulatorAttachments(): Readonly<Record<string, SimulatorAttachment>> {
    return useExternalValue(agents, (current) => current.attachments);
}

export function useSimulatorActing(agentId: string): boolean {
    return useExternalValue(agents, (current) => !!current.acting[agentId]);
}
