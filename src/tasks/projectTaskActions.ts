import { buildTools } from "../api/buildTools";
import { toolchains } from "../api/toolchains";
import type { ProjectConfigLoadResult } from "../projects/projectConfig";
import { whenTrusted } from "../projects/projectTrust";
import { notify, reportError } from "../state/toast";
import { activeProjectTaskInventoryMatches, appTaskRuntime, javaChoiceOf, refreshDetectedTasks } from "./application";
import { buildToolOf, isDetectedRunTask } from "./detectedTasks";
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
    detected.sort((left, right) => Number(isDetectedRunTask(right.id)) - Number(isDetectedRunTask(left.id)));
    return [...configured, ...detected];
}

const installing = new Set<string>();

function installThenRun(name: string, install: () => Promise<string>, run: () => void): void {
    if (installing.has(name)) {
        notify("info", `${name} is already being installed`);
        return;
    }
    installing.add(name);
    notify("info", `Installing ${name}…`);
    void install()
        .then((path) => {
            notify("success", `Installed ${name} at ${path}`);
            run();
        })
        .catch(reportError(`install ${name}`))
        .finally(() => installing.delete(name));
}

/** Offers to install the build tool a detected task needs instead of letting the shell fail with "command not found". */
function whenToolInstalled(task: ResolvedTaskDefinition, run: () => void): void {
    const tool = buildToolOf(task.command);
    if (!tool) return run();
    void buildTools
        .installed(tool.program)
        .then((installed) => {
            if (installed) return run();
            notify("error", `${task.label} needs ${tool.name}, which isn't installed.`, {
                action: {
                    label: `Install ${tool.name}`,
                    dismissOnClick: true,
                    run: () => installThenRun(tool.name, () => buildTools.install(tool.program), run),
                },
                timeoutMs: null,
            });
        })
        .catch(reportError(`check ${tool.name}`));
}

/** Detection already points the task at a new-enough Java; this covers the case where none is installed. */
function whenJavaInstalled(task: ResolvedTaskDefinition, run: () => void): void {
    const choice = javaChoiceOf(task.project, task.id);
    if (!choice || choice.runtime) return run();
    const name = `Java ${choice.required}`;
    const current = choice.current ? ` Your Java is ${choice.current.major}.` : "";
    notify("error", `${task.label} needs ${name} or newer.${current}`, {
        action: {
            label: `Install ${name}`,
            dismissOnClick: true,
            run: () =>
                installThenRun(
                    name,
                    async () => {
                        const runtime = await toolchains.installJava(choice.required);
                        await refreshDetectedTasks(task.project);
                        return runtime.home;
                    },
                    run,
                ),
        },
        timeoutMs: null,
    });
}

// Detected tasks run fixed commands Sikemux wrote, so only sikemux.json tasks need the trust prompt.
function whenTaskAllowed(project: string, config: ProjectConfigLoadResult | null, task: ResolvedTaskDefinition, run: () => void): void {
    if (task.source === "built-in") whenToolInstalled(task, () => whenJavaInstalled(task, run));
    else if (config?.status === "valid") whenTrusted(project, config, true, run);
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
