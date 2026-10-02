import { mutate } from "../store";
import { collectPanes } from "../layout";
import type { AgentSessionPlan } from "../coreSessionClaims";
import { closeAgent } from "./agents";

/** Records the core terminal a pane shows, so the next launch can take it back. */
export function setPanePty(paneId: string, ptyId: number): void {
    mutate((d) => {
        for (const window of Object.values(d.windows)) {
            const pane = collectPanes(window.root).find((candidate) => candidate.id === paneId);
            if (!pane) continue;
            if (pane.ptyId !== ptyId) pane.ptyId = ptyId;
            return;
        }
    });
}

export function setAgentPty(agentId: string, ptyId: number): void {
    mutate((d) => {
        const agent = d.agents[agentId];
        if (agent && agent.ptyId !== ptyId) agent.ptyId = ptyId;
    });
}

/**
 * Terminal agents still running in the core come back live, and so do ones
 * whose terminal crashed, to be resumed in it; the rest stay asleep without
 * their old terminal.
 */
export function applyAgentSessionPlan(plan: AgentSessionPlan): void {
    mutate((d) => {
        for (const id of [...plan.live, ...plan.resume]) {
            const agent = d.agents[id];
            if (agent) agent.launchState = "live";
        }
        for (const id of plan.gone) delete d.agents[id]?.ptyId;
    });
    for (const id of plan.dropped) closeAgent(id);
}
