import { toolchains } from "../api/toolchains";
import { joinPath } from "../lib/paths";
import type { ProjectTask } from "../projects/projectConfig";
import { openTaskTerminal } from "../state/commands";
import { readPersonalTasks } from "./addedTasks";
import { detectProjectTasks } from "./detectedTasks";
import { chooseJava, type JavaChoice } from "./javaToolchain";
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

const javaChoices = new Map<string, JavaChoice>();
let detection = 0;

export function javaChoiceOf(project: string, taskId: string): JavaChoice | null {
    return javaChoices.get(`${project}\0${taskId}`) ?? null;
}

export interface DetectedTasks {
    readonly tasks: readonly TaskDefinitionInput[];
    readonly java: ReadonlyMap<string, JavaChoice>;
}

/** A project's build-file tasks, each pointed at the Java it should run on; java is keyed by task id. */
export async function detectTasksWithJava(project: string): Promise<DetectedTasks> {
    const [detected, personal] = await Promise.all([detectProjectTasks(project), readPersonalTasks(project)]);
    const runtimes = detected.some((task) => task.java) ? await toolchains.javaRuntimes().catch(() => null) : null;
    const java = new Map<string, JavaChoice>();
    const tasks = detected.map(({ java: required, ...task }): TaskDefinitionInput => {
        if (!required || !runtimes) return task;
        const choice = chooseJava(required, runtimes);
        java.set(task.id, choice);
        if (!choice.runtime || choice.runtime.home === choice.current?.home) return task;
        return { ...task, env: { ...task.env, JAVA_HOME: choice.runtime.home } };
    });
    return { tasks: [...personal, ...tasks], java };
}

let shownProject: string | null = null;
let shownTasks: readonly TaskDefinitionInput[] = [];
const rerunTasks = new Map<string, TaskDefinitionInput>();
const portOverrides = new Map<string, { readonly envVar: string; readonly port: number }>();

function publishDetectedTasks(): void {
    const tasks = [...shownTasks, ...rerunTasks.values()].map((task) => {
        const moved = portOverrides.get(task.id);
        return moved ? { ...task, env: { ...task.env, [moved.envVar]: String(moved.port) } } : task;
    });
    appTaskRegistry.replaceSource("built-in", tasks);
}

/** Runs a detected task's app on another port from now on, for when its usual one is taken. */
export function setPortOverride(project: string, taskId: string, envVar: string, port: number): void {
    if (project !== shownProject) return;
    portOverrides.set(taskId, { envVar, port });
    publishDetectedTasks();
}

/** Shows the active project's build-file tasks; a newer call wins over one still reading. */
export async function refreshDetectedTasks(project: string | null): Promise<void> {
    const token = ++detection;
    const detected = project ? await detectTasksWithJava(project) : { tasks: [], java: new Map<string, JavaChoice>() };
    if (token !== detection) return;
    if (project !== shownProject) {
        rerunTasks.clear();
        portOverrides.clear();
    }
    shownProject = project;
    shownTasks = detected.tasks;
    javaChoices.clear();
    for (const [taskId, choice] of detected.java) javaChoices.set(`${project}\0${taskId}`, choice);
    publishDetectedTasks();
}

/** Adds a one-off task, such as a test run narrowed to the tests that failed, beside the detected ones. */
export function addRerunTask(task: TaskDefinitionInput, javaFrom: string): void {
    if (task.project !== shownProject) return;
    const java = javaChoices.get(`${task.project}\0${javaFrom}`);
    if (java) javaChoices.set(`${task.project}\0${task.id}`, java);
    rerunTasks.set(task.id, task);
    publishDetectedTasks();
}

export function activeProjectTaskInventoryMatches(project: string, configFingerprint: string): boolean {
    return activeProjectTaskInventory?.[0] === project && activeProjectTaskInventory[1] === configFingerprint;
}

export function subscribeAppTasks(listener: () => void): () => void {
    return appTaskRegistry.subscribe(listener);
}

export function subscribeTaskRuntime(listener: () => void): () => void {
    return appTaskRuntime.subscribe(listener);
}

export function getAppTaskSnapshot(): TaskRegistrySnapshot {
    return appTaskRegistry.getSnapshot();
}
