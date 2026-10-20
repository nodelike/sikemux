/** `start` opens the first page the project has. */
export type ProjectSettingsPageId =
    "start" | "overview" | "environment" | "services" | "tasks" | "done" | "brief" | "guardrails" | "suggestions" | `stack:${string}`;

/** A language's page in one folder of the project. */
export const stackPage = (folder: string, language: string): ProjectSettingsPageId => `stack:${language}:${folder}`;

export function stackOfPage(page: ProjectSettingsPageId): { language: string; folder: string } | null {
    if (!page.startsWith("stack:")) return null;
    const [language, ...folder] = page.slice("stack:".length).split(":");
    return { language, folder: folder.join(":") };
}
