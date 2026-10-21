import { keybindingLabel, matchesKeybinding } from "../commands/keybindings";
import type { StandaloneCommand } from "../commands/registry";
import * as cmd from "../state/commands";
import { activeProject } from "./projectState";
import type { ProjectAction } from "./projectConfig";
import { whenTrusted, type ValidProjectConfig } from "./projectTrust";

function projectActionCommandId(action: ProjectAction): string {
    return `project.action.${action.id}`;
}

function runProjectAction(project: string, config: ValidProjectConfig, action: ProjectAction): void {
    cmd.noteRecentCommand(`standalone:${projectActionCommandId(action)}`);
    whenTrusted(project, config, () =>
        cmd.runCustomCommand({
            id: `project.${action.id}`,
            title: action.label,
            detail: action.description,
            command: action.command,
            contexts: action.contexts,
            placement: action.placement,
        }),
    );
}

export function projectActionCommands(project: string, config: ValidProjectConfig): StandaloneCommand[] {
    return config.config.actions.map((action) => ({
        id: projectActionCommandId(action),
        title: action.label,
        detail: action.description || `Run from ${config.path}`,
        category: "Project · sikemux.json",
        shortcut: action.keybinding ? keybindingLabel(action.keybinding) : "",
        execute: () => runProjectAction(project, config, action),
    }));
}

/** Runs the active project's action bound to this key, if it has one. */
export function runProjectActionForKey(event: KeyboardEvent, project: string): boolean {
    const snapshot = activeProject();
    const config = snapshot?.cwd === project ? snapshot.config : null;
    if (config?.status !== "valid") return false;
    const action = config.config.actions.find((candidate) => candidate.keybinding && matchesKeybinding(event, candidate.keybinding));
    if (!action) return false;
    runProjectAction(project, config, action);
    return true;
}
