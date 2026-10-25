import type { CatalogAgent, ChatDriver } from "../wire.generated";
import { externalValue, useExternalValue } from "../lib/externalValue";

export type { CatalogAgent };

/** An agent's id in the core's catalog, such as `claude`. */
export type AgentType = string;

/** The agents the core said it can run, read before anything shows an agent. */
const catalog = externalValue<{ agents: readonly CatalogAgent[]; byType: ReadonlyMap<string, CatalogAgent> }>({ agents: [], byType: new Map() });

/** Takes the catalog a core sent; agents it does not list keep what an earlier core said of them. */
export function setAgentCatalog(agents: readonly CatalogAgent[]): void {
    const byType = new Map(catalog.get().byType);
    for (const agent of agents) byType.set(agent.type, agent);
    catalog.set({ agents: [...byType.values()], byType });
}

/** Draws again when the catalog changes, for views that can show before a core has sent it. */
export const useAgentCatalog = (): readonly CatalogAgent[] => useExternalValue(catalog, (value) => value.agents);

const entry = (type: string | null | undefined) => (type ? catalog.get().byType.get(type) : undefined);
const typesWhere = (keep: (agent: CatalogAgent) => boolean): AgentType[] =>
    catalog
        .get()
        .agents.filter(keep)
        .map((agent) => agent.type);

export const agentTypes = (): AgentType[] => typesWhere(() => true);

export function isAgentType(value: unknown): value is AgentType {
    return typeof value === "string" && !!entry(value);
}

/** The agent's name; anything not in the catalog keeps the id it came with. */
export function agentLabel(type: string): string {
    return entry(type)?.label ?? type;
}

/** One line on what the agent is, for someone choosing which to install. */
export function agentAbout(type: string): string | null {
    return entry(type)?.about ?? null;
}

/** The agent's colour, drawing again once a core has said it. */
export const useAgentBrand = (type: string): string | undefined => useExternalValue(catalog, (value) => value.byType.get(type)?.brand);

export function agentBrand(type: AgentType): string {
    return entry(type)!.brand;
}

/** Claude and Codex keep each sign-in in its own folder, so the person can hold several accounts. */
export function hasAccounts(type: string | null | undefined): type is AgentType {
    return entry(type)?.accounts === true;
}

export const accountAgentTypes = (): AgentType[] => typesWhere((agent) => agent.accounts);

/** Whether the app can read how much of the agent's plan is used. */
export function hasUsage(type: string | null | undefined): type is AgentType {
    return entry(type)?.usage === true;
}

export const usageAgentTypes = (): AgentType[] => typesWhere((agent) => agent.usage);

/** Whether the app can delete a saved chat from the agent's own history. */
export function canDeleteChats(type: AgentType): boolean {
    return entry(type)?.deletableChats === true;
}

/** The reasoning efforts the agent's CLI takes. */
export function agentEfforts(type: AgentType): readonly string[] {
    return entry(type)?.efforts ?? [];
}

/** Whether the agent's CLI can start without asking for permissions. */
export function canBypassPermissions(type: AgentType): boolean {
    return entry(type)?.bypass === true;
}

/** The agent's official install command, which puts it where its own updater expects. Null when it has none to run. */
export function installCommand(type: string): string | null {
    return entry(type)?.install ?? null;
}

/** The arguments that start the agent's own sign-in. Null when it has none. */
export function loginArgs(type: string): readonly string[] | null {
    return entry(type)?.login ?? null;
}

/** Whether the built-in chat can drive the agent. */
export function agentSupportsChat(type: string): type is AgentType {
    return entry(type)?.chat != null;
}

export const chatAgentTypes = (): AgentType[] => typesWhere((agent) => agent.chat !== null);

/** Which of the core's chat drivers runs the agent; null when it has no chat. */
export function chatDriver(type: AgentType): ChatDriver | null {
    return entry(type)?.chat ?? null;
}

/** The models the core can ask the agent for a commit message with, its default first; none when it writes none. */
export function commitModels(type: string): readonly string[] {
    return entry(type)?.commitModels ?? [];
}

/** The agents that write commit messages. */
export const commitWriterTypes = (): AgentType[] => typesWhere((agent) => agent.commitModels.length > 0);

/** The agents offered first, before anything else is installed. */
export const featuredAgentTypes = (): AgentType[] => typesWhere((agent) => agent.featured);

/** Whether the agent can also name a saved session by its transcript's path. */
export function resumesByPath(type: AgentType): boolean {
    return entry(type)?.resumesByPath === true;
}

/** Whether the agent's saved sessions can follow its chat into another folder. */
export function hasRelocatableSessions(type: AgentType): boolean {
    return entry(type)?.relocatableSessions === true;
}
