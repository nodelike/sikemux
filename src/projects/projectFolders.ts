import { useCallback, useEffect, useState } from "react";
import { coreCall } from "../api/core";

export interface FolderLanguage {
    readonly id: "java" | "node" | "go" | "rust" | "python";
    readonly markers: readonly string[];
    /** The version the folder's files ask for, such as `17`. */
    readonly version?: string;
    readonly framework?: "spring-boot";
}

export interface FolderEnvFile {
    /** Relative to the folder. */
    readonly path: string;
    /** Null when every environment reads it. */
    readonly environment: string | null;
    readonly local: boolean;
    readonly template: boolean;
    readonly loadedBy: readonly string[];
    readonly variables: readonly { key: string; value: string }[];
}

export interface ProjectFolder {
    /** Relative to the project, `.` for its root. */
    readonly path: string;
    readonly languages: readonly FolderLanguage[];
    readonly envFiles: readonly FolderEnvFile[];
    readonly environments: readonly { name: string; production: boolean }[];
}

export interface FolderSettings {
    /** Absent loads no files; empty loads only those every environment shares. */
    readonly environment?: string;
    readonly env?: Readonly<Record<string, string>>;
    /** The tasks an agent's work in the folder must pass before it is done, by id. */
    readonly done?: readonly string[];
    /** Whose env files agents get when not the person's; production never reaches them. */
    readonly agentEnvironment?: string;
    /** docker-compose services the folder's tasks need running first. */
    readonly services?: readonly string[];
}

/** What the person tells every agent working in the project. */
export interface ProjectRules {
    readonly notes?: string;
    readonly checkEveryFolder?: boolean;
    readonly stopOnFailure?: boolean;
    /** Path globs agents must not change without asking. */
    readonly neverEdit?: readonly string[];
    /** Command patterns an agent asks before running. */
    readonly askBefore?: readonly string[];
    /** Command patterns agents never run. */
    readonly neverRun?: readonly string[];
}

export interface ProjectFolders {
    readonly folders: readonly ProjectFolder[];
    readonly settings: Readonly<Record<string, FolderSettings>>;
    readonly rules: ProjectRules;
}

/** The project's folders as the core finds them, and the environment the person chose for each. */
export function useProjectFolders(project: string): { folders: ProjectFolders | null; refresh: () => void } {
    const [found, setFound] = useState<{ project: string; folders: ProjectFolders } | null>(null);
    const [version, setVersion] = useState(0);
    const refresh = useCallback(() => setVersion((current) => current + 1), []);
    useEffect(() => {
        let cancelled = false;
        void coreCall<ProjectFolders>("projectFolders", { project })
            .then((folders) => {
                if (!cancelled) setFound({ project, folders });
            })
            .catch(() => {});
        return () => {
            cancelled = true;
        };
    }, [project, version]);
    useEffect(() => {
        window.addEventListener("focus", refresh);
        return () => window.removeEventListener("focus", refresh);
    }, [refresh]);
    return { folders: found?.project === project ? found.folders : null, refresh };
}

export function saveProjectRules(project: string, rules: ProjectRules): Promise<void> {
    return coreCall<void>("setProjectRules", { project, rules });
}

/** What an agent working in the project is told about it, as `workspace_inspect` gives it. */
export function loadProjectBrief(project: string): Promise<string> {
    return coreCall<string>("projectBrief", { project });
}

export function saveFolderSettings(project: string, folder: string, settings: FolderSettings): Promise<void> {
    return coreCall<void>("setFolderSettings", { project, folder, settings });
}

export const LANGUAGE_NAMES: Record<FolderLanguage["id"], string> = {
    java: "Java",
    node: "Node",
    go: "Go",
    rust: "Rust",
    python: "Python",
};

export function folderName(folder: string, projectName: string): string {
    return folder === "." ? projectName : `${folder}/`;
}

const SECRET = /key|secret|token|passw|pwd|private|credential|auth|cookie|session|dsn/i;

/** A value that should stay hidden until asked for, by what its variable is called. */
export function looksSecret(key: string): boolean {
    return SECRET.test(key);
}
