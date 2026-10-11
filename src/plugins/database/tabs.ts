export type DatabaseTab =
    | { id: string; kind: "console"; profile: string; number: number }
    | { id: string; kind: "table"; profile: string; schema: string; name: string }
    | { id: string; kind: "history"; profile: string }
    | { id: string; kind: "connection"; profile: string };

export interface OpenTable {
    schema: string;
    name: string;
}

export interface TabStrip {
    tabs: DatabaseTab[];
    active: string | null;
}

export const consoleTab = (profile: string, number: number): DatabaseTab => ({
    id: `console:${profile}:${number}`,
    kind: "console",
    profile,
    number,
});

export const tableTab = (profile: string, schema: string, name: string): DatabaseTab => ({
    id: `table:${profile}:${schema}.${name}`,
    kind: "table",
    profile,
    schema,
    name,
});

export const historyTab = (profile: string): DatabaseTab => ({ id: `history:${profile}`, kind: "history", profile });

export const connectionTab = (profile: string): DatabaseTab => ({ id: `connection:${profile}`, kind: "connection", profile });

/** Shows the tab, opening it just after the one in front when it is not open yet. */
export function openTab(strip: TabStrip, tab: DatabaseTab): TabStrip {
    if (strip.tabs.some((each) => each.id === tab.id)) return { ...strip, active: tab.id };
    const at = strip.tabs.findIndex((each) => each.id === strip.active);
    const tabs = [...strip.tabs];
    tabs.splice(at < 0 ? tabs.length : at + 1, 0, tab);
    return { tabs, active: tab.id };
}

/** Closes the tab; when it was in front, its right-hand neighbour takes its place, or else its left one. */
export function closeTab(strip: TabStrip, id: string): TabStrip {
    const at = strip.tabs.findIndex((each) => each.id === id);
    if (at < 0) return strip;
    const tabs = strip.tabs.filter((each) => each.id !== id);
    if (strip.active !== id) return { tabs, active: strip.active };
    return { tabs, active: (tabs[at] ?? tabs[at - 1])?.id ?? null };
}

export function closeOtherTabs(strip: TabStrip, id: string): TabStrip {
    const kept = strip.tabs.filter((each) => each.id === id);
    return { tabs: kept, active: kept[0]?.id ?? null };
}

/** Closes every tab of one saved connection, as when it is removed. */
export function closeProfileTabs(strip: TabStrip, profile: string): TabStrip {
    return strip.tabs.filter((each) => each.profile === profile).reduce((next, tab) => closeTab(next, tab.id), strip);
}

/** A console for the connection: the one in front if it is one, else its first, else a new one. */
export function consoleFor(strip: TabStrip, profile: string): DatabaseTab {
    const consoles = strip.tabs.filter((tab) => tab.kind === "console" && tab.profile === profile);
    return consoles.find((tab) => tab.id === strip.active) ?? consoles[0] ?? consoleTab(profile, 1);
}

export function newConsole(strip: TabStrip, profile: string): DatabaseTab {
    const numbers = strip.tabs.flatMap((tab) => (tab.kind === "console" && tab.profile === profile ? [tab.number] : []));
    return consoleTab(profile, Math.max(0, ...numbers) + 1);
}

export function tabLabel(tab: DatabaseTab): string {
    switch (tab.kind) {
        case "console":
            return tab.number === 1 ? "console" : `console ${tab.number}`;
        case "table":
            return tab.name;
        case "history":
            return "history";
        case "connection":
            return "properties";
    }
}
