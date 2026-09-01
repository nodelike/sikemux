export type SettingsPageId = "general" | "agents" | "commands" | "appearance" | "keybindings" | "cli" | "cloud" | "about";

export interface SettingsPage {
    id: SettingsPageId;
    name: string;
    detail: string;
    deck: string;
}

export interface SettingsGroup {
    label: string;
    pages: SettingsPage[];
}

export const SETTINGS_GROUPS: SettingsGroup[] = [
    {
        label: "Workspace",
        pages: [
            {
                id: "general",
                name: "General",
                detail: "Projects and discovery",
                deck: "Where Sikemux looks for the projects in your session picker.",
            },
            {
                id: "agents",
                name: "Agents",
                detail: "Profiles and launch safety",
                deck: "Providers, launch safety, and what comes back after a restart.",
            },
            {
                id: "commands",
                name: "Command deck",
                detail: "Your contextual actions",
                deck: "Your own shell actions, sitting beside every built-in command.",
            },
        ],
    },
    {
        label: "Interface",
        pages: [
            {
                id: "appearance",
                name: "Appearance",
                detail: "Theme and window",
                deck: "Theme and window feel. Changes apply instantly.",
            },
            {
                id: "keybindings",
                name: "Keybindings",
                detail: "Commands and navigation",
                deck: "Every command, rebindable. Changes apply instantly.",
            },
        ],
    },
    {
        label: "System",
        pages: [
            {
                id: "cli",
                name: "CLI",
                detail: "Shell and editor integration",
                deck: "Open files and projects from your shell in the running app.",
            },
            {
                id: "cloud",
                name: "Cloud",
                detail: "Sign-in workspace",
                deck: "Where cloud sign-in links open, and where they land.",
            },
            {
                id: "about",
                name: "About",
                detail: "Updates and diagnostics",
                deck: "Updates, diagnostics, and session transfer.",
            },
        ],
    },
];

export const SETTINGS_PAGES: SettingsPage[] = SETTINGS_GROUPS.flatMap((group) => group.pages);

const PAGES_BY_ID = new Map(SETTINGS_PAGES.map((page) => [page.id, page]));

export function settingsPage(id: SettingsPageId): SettingsPage {
    const page = PAGES_BY_ID.get(id);
    if (!page) throw new Error(`unknown settings page: ${id}`);
    return page;
}

export interface SettingsSection {
    id: string;
    page: SettingsPageId;
    title: string;
    sub: string;
    /** Extra words the search should match — synonyms the visible copy never says out loud. */
    keywords: string;
    macOnly?: boolean;
}

export const SETTINGS_SECTIONS = [
    {
        id: "general.roots",
        page: "general",
        title: "Project folders",
        sub: "Scanned for git repos as deep as the depth allows. “Index itself” also offers the folder as a project of its own.",
        keywords: "projects repositories directories discovery scan depth root workspace",
    },
    {
        id: "agents.safety",
        page: "agents",
        title: "Default safety boundary",
        sub: "Shown before every launch. Providers without matching CLI controls fall back to their own settings.",
        keywords: "permissions approval normal yolo bypass sandbox claude codex gemini",
    },
    {
        id: "agents.profiles",
        page: "agents",
        title: "Provider profiles",
        sub: "Profiles choose the local provider executable used at launch. Credential values are never saved by Sikemux.",
        keywords: "executable path binary accounts claude codex gemini credentials default",
    },
    {
        id: "agents.restart",
        page: "agents",
        title: "Restart behavior",
        sub: "Only confirmed native agent session IDs are saved. Raw startup commands and terminal evidence never touch disk.",
        keywords: "restore tabs resume reopen sleep idle memory",
    },
    {
        id: "agents.density",
        page: "agents",
        title: "Rail density",
        sub: "Compact mode fits more sessions while keeping state symbols visible.",
        keywords: "sidebar rows compact comfortable spacing",
    },
    {
        id: "commands.list",
        page: "commands",
        title: "Custom actions",
        sub: "Commands run in the active session's directory with SIKEMUX_SESSION_* and SIKEMUX_PROJECT set. They are unsandboxed — only add commands you trust.",
        keywords: "shell scripts palette environment variables trust",
    },
    {
        id: "commands.editor",
        page: "commands",
        title: "Action editor",
        sub: "Choose where output should live: a terminal tab, split, temporary popup, background toast, or replacement pane.",
        keywords: "new edit delete placement context terminal split popup background replace",
    },
    {
        id: "appearance.host",
        page: "appearance",
        title: "Host appearance",
        sub: "Follow the operating system light/dark switch, or keep one theme fixed.",
        keywords: "system auto automatic light dark mode",
    },
    {
        id: "appearance.theme",
        page: "appearance",
        title: "Theme",
        sub: "Applies instantly to chrome, editor and terminal — no reload. Hover a card to customize or delete.",
        keywords: "themes colors colours palette syntax highlighting custom fork",
    },
    {
        id: "appearance.window",
        page: "appearance",
        title: "Window feel",
        sub: "Tune how much of the desktop shows through.",
        keywords: "opacity transparency translucent blur glass frosted",
        macOnly: true,
    },
    {
        id: "keybindings.map",
        page: "keybindings",
        title: "Command map",
        sub: "Select a shortcut, then press a new combination. Conflicts are blocked so every command stays reachable.",
        keywords: "shortcuts hotkeys keys bindings remap rebind reset conflicts",
    },
    {
        id: "cli.integration",
        page: "cli",
        title: "Shell integration",
        sub: "Installs the sikemux and sikemux-editor commands so a shell can drive the running app.",
        keywords: "terminal path install update EDITOR git commit wait",
    },
    {
        id: "cli.usage",
        page: "cli",
        title: "Usage",
        sub: "Existing files open in an editor tab. Project directories focus or create their workspace.",
        keywords: "examples open line column arguments",
    },
    {
        id: "cloud.browser",
        page: "cloud",
        title: "Sign-in browser",
        sub: "Where the SSO URL lands. Pick the app you actually log in with.",
        keywords: "aws gcp sso login safari arc zen chrome default",
    },
    {
        id: "cloud.workspace",
        page: "cloud",
        title: "Workspace switch",
        sub: "Fired right after the link opens — point it at the desktop where the browser lives.",
        keywords: "shortcut desktop space mission control switch",
    },
    {
        id: "about.channel",
        page: "about",
        title: "Update channel",
        sub: "Stable follows the latest signed release. Preview follows the signed moving preview release.",
        keywords: "updates upgrade version release stable preview check",
    },
    {
        id: "about.support",
        page: "about",
        title: "Support deck",
        sub: "These views are also searchable from the command deck.",
        keywords: "whats new changelog diagnostics runtime health onboarding replay",
    },
    {
        id: "about.transfer",
        page: "about",
        title: "Session transfer",
        sub: "Clipboard bundles exclude Bruno secrets, drafts, terminal history, environment values, and all startup commands. Imported agents are dormant.",
        keywords: "export import copy paste clipboard bundle move machine",
    },
] as const satisfies readonly SettingsSection[];

export type SettingsSectionId = (typeof SETTINGS_SECTIONS)[number]["id"];

const SECTIONS: readonly (SettingsSection & { id: SettingsSectionId })[] = SETTINGS_SECTIONS;

const SECTIONS_BY_ID = new Map<SettingsSectionId, SettingsSection>(SECTIONS.map((section) => [section.id, section]));

export function settingsSection(id: SettingsSectionId): SettingsSection {
    const section = SECTIONS_BY_ID.get(id);
    if (!section) throw new Error(`unknown settings section: ${id}`);
    return section;
}

const HAYSTACKS = new Map<SettingsSectionId, string>(
    SECTIONS.map((section) => [section.id, `${settingsPage(section.page).name} ${section.title} ${section.sub} ${section.keywords}`.toLowerCase()]),
);

export interface SettingsMatches {
    sections: Set<SettingsSectionId>;
    counts: Partial<Record<SettingsPageId, number>>;
}

/** Every section matches an empty query, so callers can filter on `sections` without special-casing. */
export function searchSettings(query: string, macos: boolean): SettingsMatches {
    const terms = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
    const sections = new Set<SettingsSectionId>();
    const counts: Partial<Record<SettingsPageId, number>> = {};
    for (const section of SECTIONS) {
        if (section.macOnly && !macos) continue;
        const haystack = HAYSTACKS.get(section.id) ?? "";
        if (!terms.every((term) => haystack.includes(term))) continue;
        sections.add(section.id);
        counts[section.page] = (counts[section.page] ?? 0) + 1;
    }
    return { sections, counts };
}

export function firstMatchingPage(matches: SettingsMatches): SettingsPageId | null {
    return SETTINGS_PAGES.find((page) => (matches.counts[page.id] ?? 0) > 0)?.id ?? null;
}
