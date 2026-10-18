import { buildTools } from "../api/buildTools";
import { runBackgroundCommand } from "../state/commands/customCommands";
import { toolchains } from "../api/toolchains";
import type { ProjectConfigLoadResult } from "../projects/projectConfig";
import { whenTrusted } from "../projects/projectTrust";
import { notify, reportError } from "../state/toast";
import { activeProjectTaskInventoryMatches, appTaskRuntime, javaChoiceOf, refreshDetectedTasks, setPortOverride } from "./application";
import { PERSONAL_TASK_PREFIX, trustPersonalTask } from "./addedTasks";
import { isDetectedRunTask } from "./detectedTasks";
import { checkTask, type CheckItem } from "./environmentCheck";
import { taskTerminalKey } from "./runtime";
import { isTestTask } from "./testResults";
import type { ResolvedTaskDefinition, TaskRegistrySnapshot } from "./taskRegistry";

/** The project's sikemux.json tasks, then the ones found from its build files. */
export function projectTasksFor(
    snapshot: TaskRegistrySnapshot,
    project: string,
    config: ProjectConfigLoadResult | null,
): readonly ResolvedTaskDefinition[] {
    const configured =
        config?.status === "valid" && activeProjectTaskInventoryMatches(project, config.fingerprint)
            ? snapshot.tasks.filter((task) => task.project === project && task.source === "project")
            : [];
    const detected = snapshot.tasks.filter((task) => task.project === project && task.source === "built-in");
    const rank = (task: ResolvedTaskDefinition) =>
        task.id.startsWith(PERSONAL_TASK_PREFIX) ? -1 : isDetectedRunTask(task.id) ? 0 : isTestTask(task.id) ? 1 : 2;
    detected.sort((left, right) => rank(left) - rank(right));
    return [...configured, ...detected];
}

/** A run held back by a failed check, waiting for the person to fix it or run anyway. */
export interface PendingCheck {
    readonly project: string;
    readonly task: ResolvedTaskDefinition;
    readonly items: readonly CheckItem[];
    readonly run: () => void;
}

let pendingCheck: PendingCheck | null = null;
const checkListeners = new Set<() => void>();

function setPendingCheck(next: PendingCheck | null): void {
    pendingCheck = next;
    for (const listener of [...checkListeners]) listener();
}

export function subscribePendingCheck(listener: () => void): () => void {
    checkListeners.add(listener);
    return () => {
        checkListeners.delete(listener);
    };
}

export function getPendingCheck(): PendingCheck | null {
    return pendingCheck;
}

export function dismissPendingCheck(): void {
    setPendingCheck(null);
}

export function runPendingCheckAnyway(check: PendingCheck): void {
    setPendingCheck(null);
    check.run();
}

/** Installs whatever the check found missing, moves the app off a taken port, then starts the task. */
export async function fixAndRun(check: PendingCheck): Promise<void> {
    for (const item of check.items) {
        if (item.ok) continue;
        if (item.kind === "build-tool") {
            const path = await buildTools.install(item.program);
            notify("success", `Installed ${item.name} at ${path}`);
        } else if (item.kind === "java") {
            const runtime = await toolchains.installJava(item.required);
            await refreshDetectedTasks(check.project);
            notify("success", `Installed Java ${runtime.major}`);
        } else if (item.kind === "deps") {
            await runBackgroundCommand(
                { id: "task.install-dependencies", title: item.install, detail: "", command: item.install, contexts: [], placement: "background" },
                check.task.cwd,
                true,
            );
        } else if (item.freePort !== null && item.envVar) {
            setPortOverride(check.project, check.task.id, item.envVar, item.freePort);
        }
    }
    setPendingCheck(null);
    check.run();
}

function whenEnvironmentReady(task: ResolvedTaskDefinition, run: () => void): void {
    void checkTask(task, javaChoiceOf(task.project, task.id), [taskTerminalKey(task.project, task.id)])
        .then((items) => {
            if (items.every((item) => item.ok)) return run();
            setPendingCheck({ project: task.project, task, items, run });
        })
        .catch(reportError(`check ${task.label}`));
}

// Tasks found in build files run commands Sikemux wrote; sikemux.json and .sikemux/tasks.json hold commands anyone could have written.
function whenTaskAllowed(project: string, config: ProjectConfigLoadResult | null, task: ResolvedTaskDefinition, run: () => void): void {
    if (task.id.startsWith(PERSONAL_TASK_PREFIX))
        void trustPersonalTask(task)
            .then((ok) => ok && whenEnvironmentReady(task, run))
            .catch(reportError(`check ${task.label}`));
    else if (task.source === "built-in") whenEnvironmentReady(task, run);
    else if (config?.status === "valid") whenTrusted(project, config, true, () => whenEnvironmentReady(task, run));
}

/** Starts the task, replacing whichever task the project is running. */
export function runProjectTask(project: string, config: ProjectConfigLoadResult | null, task: ResolvedTaskDefinition): void {
    whenTaskAllowed(project, config, task, () => {
        const snapshot = appTaskRuntime.getSnapshot(project);
        const operation =
            snapshot?.status === "running" || snapshot?.status === "stopping"
                ? appTaskRuntime.restart(project, task.id)
                : appTaskRuntime.run(project, task.id);
        void operation.then(() => notify("success", `Started task: ${task.label}`)).catch(reportError(`start task ${task.label}`));
    });
}

export function restartProjectTask(project: string, config: ProjectConfigLoadResult | null, task: ResolvedTaskDefinition): void {
    whenTaskAllowed(project, config, task, () => {
        void appTaskRuntime
            .restart(project, task.id)
            .then(() => notify("success", "Restarted active task"))
            .catch(reportError("restart active task"));
    });
}

export function stopProjectTask(project: string): void {
    void appTaskRuntime
        .stop(project)
        .then(() => notify("success", "Stopped active task"))
        .catch(reportError("stop active task"));
}
