import { useEffect } from "react";
import { coreCall } from "../api/core";
import { onCoreChanged } from "../api/coreChanged";
import { invalidate, resource } from "../state/resources";
import type { ProjectFolders } from "./projectFolders";
import type { Suggestion } from "./projectSuggestions";

export interface ComposeService {
    readonly name: string;
    readonly image?: string;
    readonly ports: readonly string[];
    /** As docker reports it, such as `running`; absent when it was never created. */
    readonly state?: string;
}

export interface ComposeFolder {
    readonly folder: string;
    readonly file: string;
    readonly services: readonly ComposeService[];
    /** Whether docker answered; when not, states are unknown. */
    readonly docker: boolean;
}

export interface JavaRuntime {
    readonly home: string;
    readonly major: number;
}

export const projectFoldersR = resource({
    kind: "project.folders",
    fetch: (project: string) => coreCall<ProjectFolders>("projectFolders", { project }),
    staleAfterMs: 10_000,
});

export const projectSuggestionsR = resource({
    kind: "project.suggestions",
    fetch: (project: string) => coreCall<Suggestion[]>("projectSuggestions", { project }),
    staleAfterMs: 10_000,
});

export const projectServicesR = resource({
    kind: "project.services",
    fetch: (project: string) => coreCall<ComposeFolder[]>("projectServices", { project }),
    staleAfterMs: 5_000,
});

export const projectBriefR = resource({
    kind: "project.brief",
    fetch: (project: string) => coreCall<string>("projectBrief", { project }),
    staleAfterMs: 10_000,
});

export const javaRuntimesR = resource({
    kind: "project.javaRuntimes",
    fetch: () => coreCall<{ installed: JavaRuntime[] }>("javaRuntimes").then((found) => found.installed),
    staleAfterMs: 5 * 60_000,
});

const READ_AGAIN: Record<string, readonly string[]> = {
    projectSettings: ["project.folders", "project.brief"],
    suggestions: ["project.suggestions"],
    tasks: ["project.brief"],
};

/** Reads project settings again whenever the core says they changed, from any window or agent. */
export function useProjectSettingsSync(): void {
    useEffect(() => {
        const stops = Object.entries(READ_AGAIN).map(([topic, kinds]) => onCoreChanged(topic, () => invalidate((kind) => kinds.includes(kind))));
        return () => stops.forEach((stop) => stop());
    }, []);
}

export type Guardrail = "neverEdit" | "askBefore" | "neverRun";

/** One change to a project's settings, as the core applies it to what it holds. */
export type ProjectChange =
    | { kind: "variable"; key: string; value: string | null }
    | { kind: "environment"; value: string | null }
    | { kind: "agentEnvironment"; value: string | null }
    | { kind: "done"; task: string; on: boolean }
    | { kind: "service"; name: string; on: boolean }
    | { kind: "notes"; text: string }
    | { kind: "checkEveryFolder"; on: boolean }
    | { kind: "stopOnFailure"; on: boolean }
    | { kind: "guardrail"; list: Guardrail; pattern: string; on: boolean };

/** Applies one change; `folder` is the folder a folder change is for, `.` for the project root. */
export function changeProject(project: string, change: ProjectChange, folder = "."): Promise<void> {
    return coreCall<void>("changeProjectSettings", { project, folder, change });
}
