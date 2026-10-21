import { isPluginKind, type PluginKind } from "../plugins/kinds";
import { pluginSurface } from "../plugins/registry";

/** What a built-in tab is, by its window's role, named as its tab reads. */
export const CORE_WINDOW_ROLES = {
    term: "Terminal",
    files: "Files",
    git: "Git",
    diff: "Diff",
    search: "Search",
    "ssh-config": "SSH config",
    named: "Window",
    agent: "Agent",
    shared: "Shared chat",
    board: "Agent board",
    "project-settings": "Project settings",
} as const;

export type CoreWindowRole = keyof typeof CORE_WINDOW_ROLES;
export type WindowRole = CoreWindowRole | PluginKind;

/** Each built-in pane: the tab role it opens as, and the title a new one takes. */
export const CORE_PANE_KINDS = {
    terminal: { role: "term", title: CORE_WINDOW_ROLES.term },
    editor: { role: "files", title: CORE_WINDOW_ROLES.files },
    git: { role: "git", title: CORE_WINDOW_ROLES.git },
    diff: { role: "diff", title: CORE_WINDOW_ROLES.diff },
    search: { role: "search", title: CORE_WINDOW_ROLES.search },
    agent: { role: "agent", title: CORE_WINDOW_ROLES.agent },
    /* A desk is a pane beside its agent, not a tab of its own. */
    desk: { role: "named", title: "Desk" },
    shared: { role: "shared", title: CORE_WINDOW_ROLES.shared },
    board: { role: "board", title: CORE_WINDOW_ROLES.board },
    "project-settings": { role: "project-settings", title: CORE_WINDOW_ROLES["project-settings"] },
} as const satisfies Record<string, { role: CoreWindowRole; title: string }>;

export type CorePaneKind = keyof typeof CORE_PANE_KINDS;

export function isWindowRole(value: unknown): value is WindowRole {
    return (typeof value === "string" && Object.hasOwn(CORE_WINDOW_ROLES, value)) || isPluginKind(value);
}

export function isCorePaneKind(value: unknown): value is CorePaneKind {
    return typeof value === "string" && Object.hasOwn(CORE_PANE_KINDS, value);
}

export function paneRole(kind: CorePaneKind | PluginKind): WindowRole {
    return isPluginKind(kind) ? kind : CORE_PANE_KINDS[kind].role;
}

export function windowRoleLabel(role: WindowRole): string {
    return isPluginKind(role) ? (pluginSurface(role)?.title ?? role) : CORE_WINDOW_ROLES[role];
}
