import { IS_MACOS } from "../lib/platform";

export type SettingsPageId =
    "general" | "appearance" | "keybindings" | "activity" | "about" | "agents" | "actions" | "cli" | "devices" | "cloud" | "plugins";

export const SETTINGS_GROUPS: { label: string; pages: SettingsPageId[] }[] = [
    { label: "App", pages: ["general", "appearance", "keybindings", "activity", "about"] },
    { label: "Tools", pages: ["agents", "actions", "cli", "devices", "cloud", "plugins"] },
];

export const SETTINGS_PAGE_ORDER: SettingsPageId[] = SETTINGS_GROUPS.flatMap((group) => group.pages);

export const SETTINGS_PAGE_NAMES: Record<SettingsPageId, string> = {
    general: "General",
    appearance: "Appearance",
    keybindings: "Keybindings",
    activity: "Activity",
    about: "About",
    agents: "Agents",
    actions: "Actions",
    cli: "Command line",
    devices: "Devices",
    cloud: "Cloud",
    plugins: "Plugins",
};

/**
 * One searchable place in settings. `target` names the section title or row
 * label the panel scrolls to; `filter` pre-fills the shortcut filter instead.
 */
export interface SettingsEntry {
    page: SettingsPageId;
    section: string;
    label: string;
    target: string;
    keywords?: string;
    filter?: string;
}

const section = (page: SettingsPageId, title: string, keywords?: string): SettingsEntry => ({
    page,
    section: title,
    label: title,
    target: title,
    keywords,
});
const row = (page: SettingsPageId, sectionTitle: string, label: string, keywords?: string): SettingsEntry => ({
    page,
    section: sectionTitle,
    label,
    target: label,
    keywords,
});

export const SETTINGS_INDEX: SettingsEntry[] = [
    section("plugins", "Built-in plugins", "aws bruno github actions rundeck signoz enable disable switch off turn on extensions integrations"),
    section("general", "Project folders", "repos repositories directories roots scan depth index picker"),
    section("general", "Session transfer", "export import clipboard move machine bundle copy"),

    section("appearance", "Theme", "colours colors palette dark light custom fork editor"),
    section("appearance", "Interface"),
    row("appearance", "Interface", "Text size", "font zoom scale larger smaller accessibility"),
    row("appearance", "Interface", "Pane texture", "dither dithering shader grain noise background effect"),
    row("appearance", "Interface", "Pane image", "picture photo wallpaper background dither dithered custom"),
    ...(IS_MACOS
        ? [
              section("appearance", "Window"),
              row("appearance", "Window", "Opacity", "transparency translucent see-through"),
              row("appearance", "Window", "Background blur", "vibrancy frosted glass"),
          ]
        : []),

    section("keybindings", "Shortcuts", "keybindings hotkeys keys keyboard remap"),

    section("activity", "Overview", "stats statistics analytics dashboard profile usage totals sessions tokens commits hours"),
    section("activity", "Calendar", "heatmap contributions streak days year"),
    section("activity", "By agent", "claude codex share breakdown"),
    section("activity", "By project", "repositories share breakdown"),

    section("about", "Updates", "version upgrade release"),
    row("about", "Updates", "Channel", "nightly stable prerelease beta"),
    row("about", "Updates", "Last checked", "check for updates now"),
    section("about", "Privacy", "analytics telemetry tracking"),
    row("about", "Privacy", "Share anonymous usage", "analytics telemetry tracking opt out posthog"),
    section("about", "Help", "what's new changelog diagnostics welcome onboarding"),

    section("agents", "Launch boundary", "permissions yolo sandbox safety bypass approval"),
    section("agents", "Provider profiles", "claude codex gemini accounts"),
    row("agents", "Provider profiles", "Name", "profile"),
    row("agents", "Provider profiles", "Provider", "claude codex gemini"),
    row("agents", "Provider profiles", "Executable path", "binary cli path"),
    row("agents", "Provider profiles", "Profile directory", "config account home"),
    row("agents", "Provider profiles", "Claude default", "profile"),
    row("agents", "Provider profiles", "Codex default", "profile"),
    section(
        "agents",
        "Model providers",
        "openrouter baseten together fireworks groq cerebras deepseek moonshot kimi mistral vercel api key hosted open models opencode pi omp",
    ),
    section("agents", "Sessions"),
    row("agents", "Sessions", "Restore agent tabs", "resume reopen startup"),
    row("agents", "Sessions", "Notify when an agent needs you", "notification alert badge dock permission finished"),
    row("agents", "Sessions", "Test notification", "notification sound permission send try"),
    row("agents", "Sessions", "Rail density", "compact comfortable sidebar"),
    row("agents", "Sessions", "Idle agents", "sleep memory process"),
    ...(IS_MACOS
        ? [
              section("agents", "Voice", "dictation microphone speech talk transcribe"),
              row("agents", "Voice", "Dictate with right Option", "dictation microphone speech push to talk hold"),
              row("agents", "Voice", "Speech model", "parakeet download neural engine"),
          ]
        : []),

    section("actions", "Your actions", "custom commands command deck scripts"),
    section("actions", "New action", "custom command add create"),
    row("actions", "New action", "Name", "title"),
    row("actions", "New action", "Description", "detail"),
    row("actions", "New action", "Command", "shell script"),
    row("actions", "New action", "Where output lands", "placement terminal split popup background"),
    row("actions", "New action", "Contexts", "project ssh aws bruno plugin"),

    section("cli", "Shell integration", "install terminal path sikemux-editor"),
    section("cli", "Usage", "editor git commit open"),

    section("devices", "Remote access", "phone mobile iphone android remote away connect"),
    row("devices", "Remote access", "Allow paired devices", "phone mobile remote enable turn on off background"),
    section("devices", "Pair a device", "phone mobile code pairing add connect"),
    section("devices", "Paired devices", "phone mobile revoke remove forget access watch"),

    section("cloud", "Single sign-on", "sso aws gcp login"),
    row("cloud", "Single sign-on", "Browser app", "chrome safari firefox arc"),
    row("cloud", "Single sign-on", "Workspace shortcut", "desktop space mission control"),
];

/**
 * Every word of the query has to appear somewhere in the entry. Entries whose
 * label starts with the query come first, then labels containing it.
 */
export function searchSettings(query: string, entries: SettingsEntry[]): SettingsEntry[] {
    const normalized = query.trim().toLowerCase();
    const words = normalized.split(/\s+/).filter(Boolean);
    if (!words.length) return [];
    const rank = (entry: SettingsEntry) => {
        const label = entry.label.toLowerCase();
        if (label.startsWith(normalized)) return 0;
        if (label.includes(normalized)) return 1;
        return 2;
    };
    return entries
        .filter((entry) => {
            const haystack = `${entry.label} ${entry.section} ${SETTINGS_PAGE_NAMES[entry.page]} ${entry.keywords ?? ""}`.toLowerCase();
            return words.every((word) => haystack.includes(word));
        })
        .map((entry, order) => ({ entry, order, rank: rank(entry) }))
        .sort((a, b) => a.rank - b.rank || a.order - b.order)
        .map(({ entry }) => entry);
}
