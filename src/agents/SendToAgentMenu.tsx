import { useMemo } from "react";
import { TreeContextMenu, type CtxItem } from "../rail/FileTree";
import { useResource } from "../state/resources";
import { agentCatalogR } from "../state/resources.defs";
import { useStore } from "../state/store";
import { AgentIcon } from "../ui/Icons";
import type { AgentDelivery } from "./agentInbox";
import { selectedAgentRuntimeProfiles } from "./agentProfiles";
import { agentChoices, nearestProjectSessionId, type AgentChoice } from "./agentTargets";
import { sendToAgent } from "./sendToAgent";

/** Who can take a delivery in this project, or in the nearest project when no session is named. */
export function useAgentChoices(sessionId?: string | null): AgentChoice[] {
    const profiles = useStore((s) => s.providerProfiles);
    const selections = useStore((s) => s.selectedProviderProfileIds);
    const runtimeProfiles = useMemo(() => selectedAgentRuntimeProfiles(profiles, selections), [profiles, selections]);
    const catalog = useResource(agentCatalogR, runtimeProfiles).data;
    const target = useStore((s) => (sessionId === undefined ? nearestProjectSessionId(s) : sessionId));
    const sessions = useStore((s) => s.sessions);
    const windows = useStore((s) => s.windows);
    const windowsBySession = useStore((s) => s.windowsBySession);
    const agents = useStore((s) => s.agents);
    return useMemo(
        () => (target ? agentChoices({ sessions, windows, windowsBySession, agents }, target, catalog ?? []) : []),
        [target, sessions, windows, windowsBySession, agents, catalog],
    );
}

/** Menu rows for these choices, to sit inside any context menu. The delivery is built only when a row is chosen. */
export function sendToAgentItems(choices: readonly AgentChoice[], delivery: () => AgentDelivery): CtxItem[] {
    if (choices.length === 0) return [{ label: "No agents available", disabled: true }];
    return choices.map((choice) => ({
        label: choice.label,
        icon: (
            <span className={`agent-glyph ${choice.type}`}>
                <AgentIcon type={choice.type} size={14} />
            </span>
        ),
        run: () => void sendToAgent(choice.target, delivery()),
    }));
}

export function SendToAgentMenu({
    x,
    y,
    sessionId,
    delivery,
    onClose,
    alignRight,
}: {
    x: number;
    y: number;
    /** The project whose agents are offered; left out, the nearest project's. */
    sessionId?: string | null;
    delivery: () => AgentDelivery;
    onClose: () => void;
    alignRight?: boolean;
}) {
    const choices = useAgentChoices(sessionId);
    return <TreeContextMenu x={x} y={y} items={sendToAgentItems(choices, delivery)} onClose={onClose} alignRight={alignRight} />;
}
