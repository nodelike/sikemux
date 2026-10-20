import { coreCall } from "../api/core";

export type SuggestionKind = "variable" | "environment" | "agentEnvironment" | "done" | "task" | "note" | "guardrail" | "service";

export interface SuggestedChange {
    readonly id: string;
    readonly kind: SuggestionKind;
    /** `.` for the project root. */
    readonly folder: string;
    readonly key?: string;
    readonly value?: string;
    readonly why: string;
    readonly evidence?: string;
    readonly status: "pending" | "applied" | "discarded";
}

export interface Suggestion {
    readonly id: string;
    readonly by: string;
    readonly at: number;
    readonly title: string;
    readonly changes: readonly SuggestedChange[];
}

export const isPending = (suggestion: Suggestion) => suggestion.changes.some((change) => change.status === "pending");

/** Applies the changes named and discards the rest of the suggestion. */
export function answerSuggestion(project: string, id: string, apply: readonly string[]): Promise<void> {
    return coreCall<void>("answerSuggestion", { project, id, apply });
}

const GUARDRAIL_NAMES: Record<string, string> = { neverEdit: "Never edit", askBefore: "Ask before running", neverRun: "Never run" };

/** One line saying what applying the change does. */
export function describeChange(change: SuggestedChange): string {
    const where = change.folder === "." ? "" : `${change.folder}/ · `;
    const value = change.value ?? "";
    switch (change.kind) {
        case "variable":
            return `${where}${change.key} = ${value || "(removed)"}`;
        case "environment":
            return `${where}load ${value || "the shared env files"}`;
        case "agentEnvironment":
            return `${where}agents load ${value || "the shared env files"}`;
        case "done":
            return `${where}Done means + ${value}`;
        case "task":
            return `${where}task ${change.key}: ${value}`;
        case "note":
            return `note for agents: ${value}`;
        case "guardrail":
            return `${GUARDRAIL_NAMES[change.key ?? ""] ?? change.key}: ${value}`;
        case "service":
            return `${where}needs ${value} running`;
    }
}

/** What an agent is asked to set the project up: read it, check by running it, and suggest everything at once. */
export function setupRequest(name: string): string {
    return [
        `Set up ${name} in Sikemux. Call guide with the setup topic first.`,
        "Read the README, AGENTS.md, the build files, scripts, env loaders, docker-compose and CI workflows,",
        "then run the build and the tests to check what you found.",
        "Send everything in one project_suggest: each folder's build settings, which environment it loads (never a production one for agents),",
        "the tasks people really run, Done means, the services it needs, and guardrails for files or commands agents must not touch.",
        "Do not change any settings or project files yourself, and never put a secret value in a suggestion.",
    ].join("\n");
}
