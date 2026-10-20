import { notify, reportError } from "../state/toast";
import { tasksApi, useTaskRuns, type TaskRun } from "../tasks/coreTasks";
import type { FolderSettings, ProjectFolder } from "./projectFolders";
import type { JavaRuntime } from "./projectResources";

export type CheckState = "never" | "running" | "passed" | "failed";

export interface CheckResult {
    readonly state: CheckState;
    /** Such as `142 passed` or `2 of 318 failed`. */
    readonly detail: string;
    readonly at: number | null;
    readonly run: TaskRun | null;
}

/** Finished runs outlive the core's list of them here, so health still shows after the core forgets a run. */
const remembered = new Map<string, CheckResult>();
const key = (project: string, taskId: string) => `${project}\u0000${taskId}`;

function resultOf(run: TaskRun): CheckResult {
    if (run.status === "completed" || run.status === "failed" || run.status === "stopped") {
        const passed = run.status === "completed" && (run.exitCode ?? 0) === 0 && !run.tests?.failed;
        const tests = run.tests;
        const detail = tests
            ? tests.failed
                ? `${tests.failed} of ${tests.passed + tests.failed} failed`
                : `${tests.passed} passed`
            : passed
              ? "passed"
              : run.status === "stopped"
                ? "stopped"
                : `exit ${run.exitCode ?? "?"}`;
        return { state: passed ? "passed" : "failed", detail, at: run.startedAt ?? Date.now(), run };
    }
    return { state: "running", detail: "running", at: run.startedAt ?? null, run };
}

/** The latest result of each Done means task of the project, by task id. */
export function useCheckResults(project: string, taskIds: readonly string[]): ReadonlyMap<string, CheckResult> {
    const runs = useTaskRuns();
    const results = new Map<string, CheckResult>();
    for (const taskId of taskIds) {
        const latest = runs
            .filter((run) => run.taskId === taskId && run.project === project)
            .sort((a, b) => (b.startedAt ?? 0) - (a.startedAt ?? 0))[0];
        const result = latest ? resultOf(latest) : remembered.get(key(project, taskId));
        if (latest && result && result.state !== "running") remembered.set(key(project, taskId), result);
        results.set(taskId, result ?? { state: "never", detail: "not run yet", at: null, run: null });
    }
    return results;
}

/** Starts every Done means task once, as the person would from the run menu. */
export async function checkSetup(project: string, taskIds: readonly string[]): Promise<void> {
    for (const taskId of taskIds) {
        const started = await tasksApi.start(project, taskId).catch((error: unknown) => {
            reportError(`Start ${taskId}`)(error);
            return null;
        });
        if (started && "checks" in started) notify("error", `${taskId} could not start: ${started.checks.map((check) => check.title).join("; ")}`);
    }
}

/** A setting that no longer matches what the folder's own files ask for, with the change that fixes it. */
export interface Drift {
    readonly folder: string;
    readonly language: string;
    readonly title: string;
    readonly detail: string;
    readonly fix: { label: string; key: string; value: string | null };
}

export function driftsOf(
    folders: readonly ProjectFolder[],
    settings: Readonly<Record<string, FolderSettings>>,
    runtimes: readonly JavaRuntime[] | null,
): Drift[] {
    const drifts: Drift[] = [];
    for (const folder of folders) {
        const env = settings[folder.path]?.env ?? {};
        const where = folder.path === "." ? "this project" : `${folder.path}/`;
        for (const language of folder.languages) {
            const asked = language.version;
            if (!asked) continue;
            if (language.id === "java" && env.JAVA_HOME && runtimes) {
                const current = runtimes.find((runtime) => runtime.home === env.JAVA_HOME)?.major;
                if (current === undefined || String(current) === asked) continue;
                const matching = runtimes.find((runtime) => String(runtime.major) === asked);
                drifts.push({
                    folder: folder.path,
                    language: "java",
                    title: `The build now asks for Java ${asked}`,
                    detail: `${where} still runs on Java ${current}, chosen here.`,
                    fix: matching
                        ? { label: `Use Java ${asked}`, key: "JAVA_HOME", value: matching.home }
                        : { label: "Choose automatically", key: "JAVA_HOME", value: null },
                });
            }
            if (language.id === "rust" && env.RUSTUP_TOOLCHAIN && env.RUSTUP_TOOLCHAIN !== asked) {
                drifts.push({
                    folder: folder.path,
                    language: "rust",
                    title: `The project now asks for Rust ${asked}`,
                    detail: `${where} still uses the ${env.RUSTUP_TOOLCHAIN} toolchain, chosen here.`,
                    fix: { label: `Use ${asked}`, key: "RUSTUP_TOOLCHAIN", value: asked },
                });
            }
        }
    }
    return drifts;
}
