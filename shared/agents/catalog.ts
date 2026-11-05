import type { CatalogAgent } from "../wire.generated";
import { externalValue, useExternalValue } from "../lib/externalValue";

export type { CatalogAgent };

/** An agent's id in the core's catalog, such as `claude`. */
export type AgentType = string;

/** The agents each core said it can run, by core, read before anything shows an agent. */
const catalogs = externalValue<ReadonlyMap<string, ReadonlyMap<string, CatalogAgent>>>(new Map());

/** Takes the catalog `core` sent, in place of what it said before. */
export function setAgentCatalog(core: string, agents: readonly CatalogAgent[]): void {
    catalogs.set(new Map(catalogs.get()).set(core, new Map(agents.map((agent) => [agent.type, agent]))));
}

export function forgetAgentCatalog(core: string): void {
    if (!catalogs.get().has(core)) return;
    const next = new Map(catalogs.get());
    next.delete(core);
    catalogs.set(next);
}

/* An agent is named and coloured alike on every core, so where no core is in view any core's word for it does. */
function anyEntry(byCore: ReadonlyMap<string, ReadonlyMap<string, CatalogAgent>>, type: string): CatalogAgent | undefined {
    for (const byType of byCore.values()) {
        const agent = byType.get(type);
        if (agent) return agent;
    }
    return undefined;
}

const entry = (type: string | null | undefined) => (type ? anyEntry(catalogs.get(), type) : undefined);

/** The agent's name; anything not in the catalog keeps the id it came with. */
export function agentLabel(type: string): string {
    return entry(type)?.label ?? type;
}

/** The company whose servers the agent's turns go to, when it has one. */
export function agentMaker(type: string): string | undefined {
    return entry(type)?.maker ?? undefined;
}

/** Whether how much of the agent's plan is used can be read, which earns its context ring the agent's own colour. */
export function agentReadsUsage(type: string): boolean {
    return entry(type)?.usage === true;
}

/** The agent's colour, drawing again once a core has said it. */
export const useAgentBrand = (type: string): string | undefined => useExternalValue(catalogs, (byCore) => anyEntry(byCore, type)?.brand);

/* The core switches a running chat between safe and YOLO for every agent whose
   CLI on it can start without asking: in the CLI's own modes where it has them,
   or else by answering the agent's permission requests itself. */
export function canSwitchMode(type: AgentType, core: string): boolean {
    return catalogs.get().get(core)?.get(type)?.bypass === true;
}
