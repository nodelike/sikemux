export type ProjectSettingsPageId = "general" | "agents" | "file";

export const PROJECT_SETTINGS_PAGES: readonly { id: ProjectSettingsPageId; name: string }[] = [
    { id: "general", name: "General" },
    { id: "agents", name: "Agents" },
    { id: "file", name: "Project file" },
];
