import { AGENT_CATALOG } from "./generated/catalog";

type Entry = (typeof AGENT_CATALOG)[number];

export type AgentType = Entry["type"];
export type AccountAgentType = Extract<Entry, { accounts: true }>["type"];
export type UsageAgentType = Extract<Entry, { usage: true }>["type"];

export const AGENT_TYPES: readonly AgentType[] = AGENT_CATALOG.map((agent) => agent.type);

const BY_TYPE: ReadonlyMap<string, Entry> = new Map(AGENT_CATALOG.map((agent) => [agent.type, agent]));

export function isAgentType(value: unknown): value is AgentType {
    return typeof value === "string" && BY_TYPE.has(value);
}

/** The agent's name; anything not in the catalog keeps the id it came with. */
export function agentLabel(type: string): string {
    return BY_TYPE.get(type)?.label ?? type;
}

/** One line on what the agent is, for someone choosing which to install. */
export function agentAbout(type: string): string | null {
    return BY_TYPE.get(type)?.about ?? null;
}

export function agentBrand(type: AgentType): string {
    return BY_TYPE.get(type)!.brand;
}

export const AGENT_BRANDS = Object.fromEntries(AGENT_CATALOG.map((agent) => [agent.type, agent.brand])) as Readonly<Record<AgentType, string>>;

/** Claude and Codex keep each sign-in in its own folder, so the person can hold several accounts. */
export function hasAccounts(type: string | null | undefined): type is AccountAgentType {
    return !!type && BY_TYPE.get(type)?.accounts === true;
}

export const ACCOUNT_AGENT_TYPES = AGENT_TYPES.filter(hasAccounts);

/** Whether the app can read how much of the agent's plan is used. */
export function hasUsage(type: string | null | undefined): type is UsageAgentType {
    return !!type && BY_TYPE.get(type)?.usage === true;
}

/** Whether the app can delete a saved chat from the agent's own history. */
export function canDeleteChats(type: AgentType): boolean {
    return BY_TYPE.get(type)?.deletableChats === true;
}

/** Whether the agent's CLI can start without asking for permissions. */
export function canBypassPermissions(type: AgentType): boolean {
    return BY_TYPE.get(type)?.bypass === true;
}

/** The agent's official install command, which puts it where its own updater expects. Null when it has none to run. */
export function installCommand(type: string): string | null {
    return BY_TYPE.get(type)?.install ?? null;
}

/** The arguments that start the agent's own sign-in. Null when it has none. */
export function loginArgs(type: string): readonly string[] | null {
    return BY_TYPE.get(type)?.login ?? null;
}
