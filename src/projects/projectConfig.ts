import { coreCall } from "../api/core";
import type { TrustAsk } from "../tasks/coreTasks";

export const PROJECT_CONFIG_VERSION = 1 as const;

export type ProjectActionPlacement = "background" | "terminal" | "split" | "popup" | "replace";
export type ProjectCommandContext = "project";

export interface ProjectAction {
    id: string;
    label: string;
    description: string;
    command: string;
    placement: ProjectActionPlacement;
    /** An empty list makes the action available throughout its owning project. */
    contexts: ProjectCommandContext[];
    keybinding?: string;
}

export interface ProjectPreview {
    /** An absolute HTTP(S) URL. Usually points at a local development server. */
    url?: string;
    /** Optional trusted project command that starts the preview server. */
    command?: string;
}

export interface ProjectTask {
    id: string;
    label: string;
    command: string;
    /** Normalized project-relative directory. `.` means the project root. */
    cwd: string;
    env: Record<string, string>;
}

export interface ProjectWorktreeCreateHook {
    id: string;
    label: string;
    command: string;
}

export interface SikemuxProjectConfig {
    version: typeof PROJECT_CONFIG_VERSION;
    /** Optional schema hint for editors. Sikemux does not fetch this URL. */
    $schema?: string;
    /** Project-relative image path. */
    icon?: string;
    actions: ProjectAction[];
    tasks: ProjectTask[];
    preview?: ProjectPreview;
    worktree?: {
        onCreate: ProjectWorktreeCreateHook[];
    };
    /** What rebuilds the committed files made from others, and those files. */
    generate?: {
        command: string;
        outputs: string[];
    };
    /** What proves the project sound: run on each commit, and on a card's change when its pull request has no CI. */
    check?: string;
}

export type ProjectConfigValidationCode =
    | "invalid-json"
    | "invalid-type"
    | "missing-field"
    | "unknown-field"
    | "unsupported-version"
    | "invalid-value"
    | "duplicate-id"
    | "limit-exceeded"
    | "read-failed";

export interface ProjectConfigValidationError {
    /** JSONPath-like location suitable for showing directly in project settings. */
    path: string;
    code: ProjectConfigValidationCode;
    message: string;
}

export interface ProjectConfigTrustSummary {
    requiresApproval: boolean;
    executableEntries: number;
    reasons: string[];
}

export type ProjectConfigLoadResult =
    | { status: "absent"; path: string }
    | {
          status: "invalid";
          path: string;
          errors: ProjectConfigValidationError[];
          /** Present when a file was read, so a changed invalid file can still be detected. */
          fingerprint?: string;
      }
    | {
          status: "valid";
          path: string;
          config: SikemuxProjectConfig;
          fingerprint: string;
          trust: ProjectConfigTrustSummary;
          /** The person trusts this file on the host, or it runs nothing. */
          trusted: boolean;
          trustAsk?: TrustAsk;
      };

/** The project's sikemux.json as the core reads and checks it. */
export function loadProjectConfig(project: string): Promise<ProjectConfigLoadResult> {
    return coreCall<ProjectConfigLoadResult>("projectConfig", { project });
}
