export type ProjectSettingsPageId = "general" | "agents";

export const PROJECT_SETTINGS_PAGES: readonly { id: ProjectSettingsPageId; name: string }[] = [
    { id: "general", name: "General" },
    { id: "agents", name: "Agents" },
];
