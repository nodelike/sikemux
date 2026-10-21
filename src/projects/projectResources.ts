import { useEffect } from "react";
import { coreCall } from "../api/core";
import { onCoreChanged } from "../api/coreChanged";
import type { CoreChangeTopic } from "../api/generated/wire";
import { subscribe } from "../state/bus";
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

const READ_AGAIN = {
    projectSettings: ["project.folders", "project.brief"],
    suggestions: ["project.suggestions"],
    tasks: ["project.brief"],
} satisfies Partial<Record<CoreChangeTopic, readonly string[]>>;

/** Files that make a folder's environments, so a new `.env.prod` shows without reopening the project. */
const ENVIRONMENT_FILE = /(^|\/)(\.env[^/]*|[^/]*\.env|\.envrc|package\.json|Makefile|[^/]*compose[^/]*\.ya?ml)$/i;

/** Reads project settings again whenever the core says they changed, from any window or agent, or an environment file changes. */
export function useProjectSettingsSync(): void {
    useEffect(() => {
        const stops = (Object.entries(READ_AGAIN) as [CoreChangeTopic, readonly string[]][]).map(([topic, kinds]) =>
            onCoreChanged(topic, () => invalidate((kind) => kinds.includes(kind))),
        );
        stops.push(
            subscribe("fs-changed", (event) => {
                if (event.paths?.length && !event.paths.some((path) => ENVIRONMENT_FILE.test(path))) return;
                invalidate((kind, args) => {
                    const project = args[0];
                    return (
                        (kind === "project.folders" || kind === "project.brief") &&
                        typeof project === "string" &&
                        (project.startsWith(event.repo) || event.repo.startsWith(project))
                    );
                });
            }),
        );
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
    return coreCall<void>("changeProjectSettings", { project, folder, change }).then(() =>
        invalidate((kind) => READ_AGAIN.projectSettings.includes(kind)),
    );
}
