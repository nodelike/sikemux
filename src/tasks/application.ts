import { joinPath } from "../lib/paths";
import type { ProjectTask } from "../projects/projectConfig";
import { openTaskTerminal } from "../state/commands";
import { TaskRegistry, type TaskDefinitionInput, type TaskRegistrySnapshot } from "./taskRegistry";
import { TaskRuntime } from "./runtime";
import { NativeTaskExecutionBackend, WorkbenchTaskTerminalSurface, taskPtyBindings } from "./nativeRuntime";

export function projectTaskDefinitions(project: string, tasks: readonly ProjectTask[]): readonly TaskDefinitionInput[] {
    return Object.freeze(
        tasks.map((task) =>
            Object.freeze({
                id: task.id,
                label: task.label,
                project,
                command: task.command,
                cwd: task.cwd === "." ? project : joinPath(project, task.cwd),
                env: Object.freeze({ ...task.env }),
            }),
        ),
    );
}

export const appTaskRegistry = new TaskRegistry();
export const appTaskRuntime = new TaskRuntime({
    registry: appTaskRegistry,
    backend: new NativeTaskExecutionBackend(),
    surface: new WorkbenchTaskTerminalSurface(taskPtyBindings, openTaskTerminal),
});

type ActiveProjectTaskInventory = readonly [project: string, configFingerprint: string];

let activeProjectTaskInventory: ActiveProjectTaskInventory | null = null;

function updateActiveProjectTasks(inventory: ActiveProjectTaskInventory | null, tasks: readonly TaskDefinitionInput[]): void {
    const previous = activeProjectTaskInventory;
    activeProjectTaskInventory = inventory;
    try {
        appTaskRegistry.replaceSource("project", tasks);
    } catch (error) {
        activeProjectTaskInventory = previous;
        throw error;
    }
}

/**
 * The registry publishes synchronously, so install its identity before the
 * definitions. Subscribers can never observe new tasks with an old identity.
 */
export function replaceActiveProjectTasks(project: string, configFingerprint: string, tasks: readonly ProjectTask[]): void {
    updateActiveProjectTasks(Object.freeze([project, configFingerprint]), projectTaskDefinitions(project, tasks));
}

export function clearActiveProjectTasks(): void {
    updateActiveProjectTasks(null, []);
}

export function activeProjectTaskInventoryMatches(project: string, configFingerprint: string): boolean {
    return activeProjectTaskInventory?.[0] === project && activeProjectTaskInventory[1] === configFingerprint;
}

export function subscribeAppTasks(listener: () => void): () => void {
    return appTaskRegistry.subscribe(listener);
}

export function getAppTaskSnapshot(): TaskRegistrySnapshot {
    return appTaskRegistry.getSnapshot();
}
