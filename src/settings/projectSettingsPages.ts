export type ProjectSettingsPageId = "general" | "agents" | "file" | `folder:${string}`;

export const PROJECT_SETTINGS_PAGES: readonly { id: ProjectSettingsPageId; name: string }[] = [
    { id: "general", name: "General" },
    { id: "agents", name: "Agents" },
    { id: "file", name: "Project file" },
];

export const folderPage = (folder: string): ProjectSettingsPageId => `folder:${folder}`;

export function folderOfPage(page: ProjectSettingsPageId): string | null {
    return page.startsWith("folder:") ? page.slice("folder:".length) : null;
}
