import { useEffect, useState, useSyncExternalStore } from "react";
import { openPortExternally, revealPortOwner } from "../ports/portActions";
import { taskTerminalReveal } from "../ports/projectPorts";
import { useProjectPorts } from "../ports/useProjectPorts";
import { projectControllerBridge } from "../projects/controllerBridge";
import { invokeCommand } from "../api/invoke";
import { getState } from "../state/store";
import { reportError } from "../state/toast";
import { IconCheck, IconChevron, IconClose, IconExternal, IconRefresh, IconRun, IconStop, IconTerminal } from "../ui/Icons";
import { Tooltip } from "../ui/Tooltip";
import { addRerunTask, appTaskRuntime, getAppTaskSnapshot, javaChoiceOf, subscribeAppTasks, subscribeTaskRuntime } from "./application";
import { checkItemText } from "./environmentCheck";
import {
    dismissPendingCheck,
    fixAndRun,
    getPendingCheck,
    projectTasksFor,
    restartProjectTask,
    runPendingCheckAnyway,
    runProjectTask,
    stopProjectTask,
    subscribePendingCheck,
    type PendingCheck,
} from "./projectTaskActions";
import { taskTerminalKey } from "./runtime";
import { TestResultsCard, useFinishedTestRun, type FinishedTestRun } from "./TestResultsCard";
import { rerunFailedCommand } from "./testResults";
import type { ResolvedTaskDefinition, TaskControllerSnapshot } from "./taskRegistry";

function useSecondsSince(runId: number | null): number {
    const [startedAt, setStartedAt] = useState(() => Date.now());
    const [now, setNow] = useState(() => Date.now());
    useEffect(() => {
        if (runId === null) return;
        const at = Date.now();
        setStartedAt(at);
        setNow(at);
        const timer = setInterval(() => setNow(Date.now()), 1000);
        return () => clearInterval(timer);
    }, [runId]);
    return Math.max(0, Math.floor((now - startedAt) / 1000));
}

function exitSummary(snapshot: TaskControllerSnapshot): string {
    const message = snapshot.failures.at(-1)?.message ?? "";
    const code = /exited with code (\d+)/.exec(message)?.[1];
    return code ? `exited ${code}` : "failed";
}

const BUILD_TOOL_LABELS: Readonly<Record<string, string>> = {
    "./mvnw": "Maven wrapper",
    mvn: "Maven",
    "./gradlew": "Gradle wrapper",
    gradle: "Gradle",
    go: "Go",
};

interface TaskGroup {
    readonly key: string;
    readonly title: string;
    readonly meta: string | null;
    readonly tasks: readonly ResolvedTaskDefinition[];
    /** The label without the folder, since the group's heading names it. */
    readonly shortLabel: (task: ResolvedTaskDefinition) => string;
}

/** sikemux.json's tasks, then one group per folder the build-file tasks came from. */
function taskGroups(tasks: readonly ResolvedTaskDefinition[], project: string): TaskGroup[] {
    const groups: TaskGroup[] = [];
    const configured = tasks.filter((task) => task.source !== "built-in");
    if (configured.length) groups.push({ key: "config", title: "sikemux.json", meta: null, tasks: configured, shortLabel: (task) => task.label });
    const byFolder = new Map<string, ResolvedTaskDefinition[]>();
    for (const task of tasks.filter((candidate) => candidate.source === "built-in"))
        byFolder.set(task.cwd, [...(byFolder.get(task.cwd) ?? []), task]);
    for (const [cwd, folderTasks] of byFolder) {
        const folder = cwd === project ? (project.split("/").pop() ?? project) : cwd.slice(project.length + 1);
        const tool = BUILD_TOOL_LABELS[folderTasks[0].command.split(" ")[0]];
        const java = javaChoiceOf(project, folderTasks[0].id);
        const javaText = java ? (java.runtime ? `Java ${java.runtime.major}` : `needs Java ${java.required}`) : null;
        const suffix = cwd === project ? "" : ` (${folder})`;
        groups.push({
            key: cwd,
            title: folder,
            meta: [tool, javaText].filter(Boolean).join(" · ") || null,
            tasks: folderTasks,
            shortLabel: (task) => (suffix && task.label.endsWith(suffix) ? task.label.slice(0, -suffix.length) : task.label),
        });
    }
    return groups;
}

/** Stops a run from the menu's Running now list: the person's own task through the runtime, an agent's through the harness. */
function stopRun(terminalKey: string | undefined, executionId: string): void {
    let key: unknown[] = [];
    try {
        key = terminalKey ? (JSON.parse(terminalKey) as unknown[]) : [];
    } catch {
        key = [];
    }
    if (key[0] === "task" && typeof key[1] === "string") {
        stopProjectTask(key[1]);
        return;
    }
    void invokeCommand("harness_stop_runs", { executionId }).catch(reportError("stop run"));
}

function CheckCard({ check }: { check: PendingCheck }) {
    const [fixing, setFixing] = useState(false);
    const installs = check.items.some((item) => !item.ok && item.kind !== "port");
    const fixable = installs || check.items.some((item) => !item.ok && item.kind === "port" && item.freePort !== null);
    const fix = () => {
        setFixing(true);
        void fixAndRun(check)
            .catch(reportError(`fix ${check.task.label}`))
            .finally(() => setFixing(false));
    };
    return (
        <>
            <div className="env-dd-scrim" onClick={dismissPendingCheck} />
            <div className="env-dd-menu tb-run-check" role="dialog" aria-label={`Before running ${check.task.label}`} data-overlay>
                <div className="tb-run-check-head">
                    <span>{check.task.label} can't start yet</span>
                    <button className="tb-run-check-close" onClick={dismissPendingCheck} aria-label="Close">
                        <IconClose size={11} />
                    </button>
                </div>
                {check.items.map((item) => {
                    const { title, detail } = checkItemText(item);
                    return (
                        <div key={item.kind} className={`tb-run-check-item${item.ok ? "" : " bad"}`}>
                            <span className="tb-run-check-mark">{item.ok ? <IconCheck size={11} /> : <IconClose size={11} />}</span>
                            <span className="tb-run-check-text">
                                <span>{title}</span>
                                <span className="tb-run-check-detail">{detail}</span>
                            </span>
                        </div>
                    );
                })}
                <div className="tb-run-check-actions">
                    <button className="tb-run-check-btn" onClick={() => runPendingCheckAnyway(check)} disabled={fixing}>
                        Run anyway
                    </button>
                    {fixable && (
                        <button className="tb-run-check-btn primary" onClick={fix} disabled={fixing}>
                            {fixing ? "Fixing…" : installs ? "Install and run" : "Fix and run"}
                        </button>
                    )}
                </div>
            </div>
        </>
    );
}

export function RunChip({ sessionId, project }: { sessionId: string; project: string }) {
    const registry = useSyncExternalStore(subscribeAppTasks, getAppTaskSnapshot);
    const controller = useSyncExternalStore(projectControllerBridge.subscribe, projectControllerBridge.getActiveSnapshot);
    const runtime = useSyncExternalStore(subscribeTaskRuntime, () => appTaskRuntime.getSnapshot(project));
    const ports = useProjectPorts(sessionId);
    const [picked, setPicked] = useState<Readonly<Record<string, string>>>({});
    const [open, setOpen] = useState(false);
    const elapsed = useSecondsSince(runtime?.activeRunId ?? null);
    const pendingCheck = useSyncExternalStore(subscribePendingCheck, getPendingCheck);
    const testRun = useFinishedTestRun(runtime);
    const [dismissedTests, setDismissedTests] = useState<string | null>(null);

    const config = controller?.cwd === project ? (controller.config ?? null) : null;
    const tasks = projectTasksFor(registry, project, config);
    const running = runtime?.activeRunId != null ? (tasks.find((task) => task.id === runtime.task?.id) ?? null) : null;
    const selected = running ?? tasks.find((task) => task.id === picked[project]) ?? tasks[0];
    if (!selected) return null;

    const key = taskTerminalKey(project, selected.id);
    const port = running ? ports.find((candidate) => candidate.owner.terminalKey === key) : undefined;
    const failed = !running && runtime?.status === "failed" && runtime.task?.id === selected.id ? exitSummary(runtime) : null;
    const java = javaChoiceOf(project, selected.id);
    const url = port ? `http://localhost:${port.port}` : null;
    const otherRuns = ports.filter((candidate) => candidate.owner.kind !== "terminal" && candidate !== port);
    const tests = !running && testRun?.task.id === selected.id ? testRun.results : null;
    const testsCardOpen =
        testRun !== null && tests !== null && tests.failed > 0 && dismissedTests !== testRun.key && !open && pendingCheck?.project !== project;

    const close = () => setOpen(false);
    const showOutput = () => {
        const reveal = taskTerminalReveal(getState(), sessionId, key);
        if (reveal) revealPortOwner(reveal);
    };
    const primaryLabel = running ? `Stop ${running.label}` : `Run ${selected.label}`;
    const state = port ? "up" : running ? "running" : tests ? (tests.failed ? "failed" : "up") : failed ? "failed" : "idle";
    const rerunFailed = (run: FinishedTestRun) => {
        const command = rerunFailedCommand(run.task.command, run.results.failures);
        if (!command) return null;
        return () => {
            setDismissedTests(run.key);
            const [base, folder] = run.task.id.split(":");
            const id = base.endsWith(".failed") ? run.task.id : folder ? `${base}.failed:${folder}` : `${base}.failed`;
            const label = run.task.label.endsWith(" (failed only)") ? run.task.label : `${run.task.label} (failed only)`;
            addRerunTask({ id, label, project, command, cwd: run.task.cwd, env: run.task.env }, run.task.id);
            const task = getAppTaskSnapshot().tasks.find((candidate) => candidate.project === project && candidate.id === id);
            if (!task) return;
            setPicked((current) => ({ ...current, [project]: id }));
            runProjectTask(project, config, task);
        };
    };
    return (
        <>
            <span className="tb-run" data-no-window-drag>
                <Tooltip label={primaryLabel}>
                    <button
                        className={`tb-run-btn${running ? " running" : ""}`}
                        onClick={() => (running ? stopProjectTask(project) : runProjectTask(project, config, selected))}
                        aria-label={primaryLabel}>
                        {running ? <IconStop size={11} /> : <IconRun size={11} />}
                    </button>
                </Tooltip>
                <button
                    className="tb-run-chip"
                    onClick={() => setOpen((value) => !value)}
                    aria-haspopup="menu"
                    aria-expanded={open}
                    aria-label="Choose a task to run">
                    {state !== "idle" && <span className={`tb-run-state ${state}`} />}
                    <span className="tb-run-label">{selected.label}</span>
                    {state === "up" && <span className="tb-run-meta">:{port!.port}</span>}
                    {state === "running" && <span className="tb-run-meta">{elapsed}s</span>}
                    {tests ? (
                        <span className={`tb-run-meta ${tests.failed ? "failed" : "passed"}`}>
                            {tests.failed ? `${tests.failed} failed` : `${tests.passed} passed`}
                        </span>
                    ) : (
                        state === "failed" && <span className="tb-run-meta failed">{failed}</span>
                    )}
                    {java && (
                        <span className={`tb-run-meta${java.runtime ? "" : " warn"}`}>
                            {java.runtime ? `Java ${java.runtime.major}` : `needs Java ${java.required}`}
                        </span>
                    )}
                    <IconChevron size={10} className="env-dd-chev" />
                </button>
                {(failed || tests?.failed) && (
                    <Tooltip label={tests?.failed ? "Show the failed tests" : "Show the output"}>
                        <button className="tb-run-why" onClick={tests?.failed ? () => setDismissedTests(null) : showOutput}>
                            Why?
                        </button>
                    </Tooltip>
                )}
                {pendingCheck?.project === project && !open && <CheckCard check={pendingCheck} />}
                {testsCardOpen && (
                    <TestResultsCard
                        run={testRun}
                        onClose={() => setDismissedTests(testRun.key)}
                        onRerunFailed={rerunFailed(testRun)}
                        onShowOutput={() => {
                            setDismissedTests(testRun.key);
                            showOutput();
                        }}
                    />
                )}
                {open && (
                    <>
                        <div className="env-dd-scrim" onClick={close} />
                        <div
                            className="env-dd-menu tb-run-menu"
                            role="menu"
                            aria-label="Tasks"
                            data-overlay
                            onKeyDown={(event) => {
                                if (event.key === "Escape") close();
                            }}>
                            {running && (
                                <>
                                    <div className="tb-run-menu-head">
                                        <span>{running.label}</span>
                                        <span className="tb-run-menu-head-meta">{port ? `up · :${port.port}` : `running · ${elapsed}s`}</span>
                                    </div>
                                    {url && (
                                        <button
                                            className="env-dd-item"
                                            role="menuitem"
                                            onClick={() => {
                                                close();
                                                openPortExternally(url);
                                            }}>
                                            <IconExternal size={12} />
                                            <span>Open localhost:{port!.port}</span>
                                        </button>
                                    )}
                                    <button
                                        className="env-dd-item"
                                        role="menuitem"
                                        onClick={() => {
                                            close();
                                            restartProjectTask(project, config, running);
                                        }}>
                                        <IconRefresh size={12} />
                                        <span>Restart</span>
                                    </button>
                                </>
                            )}
                            {(running || failed) && (
                                <button
                                    className="env-dd-item"
                                    role="menuitem"
                                    onClick={() => {
                                        close();
                                        showOutput();
                                    }}>
                                    <IconTerminal size={12} />
                                    <span>Show output</span>
                                </button>
                            )}
                            {taskGroups(tasks, project).map((group) => (
                                <div key={group.key} role="group" aria-label={group.title}>
                                    <div className="tb-run-menu-head">
                                        <span>{group.title}</span>
                                        {group.meta && <span className="tb-run-menu-head-meta">{group.meta}</span>}
                                    </div>
                                    {group.tasks.map((task) => (
                                        <button
                                            key={task.id}
                                            className={`env-dd-item${task.id === selected.id ? " active" : ""}`}
                                            role="menuitemradio"
                                            aria-checked={task.id === selected.id}
                                            onClick={() => {
                                                close();
                                                setPicked((current) => ({ ...current, [project]: task.id }));
                                            }}>
                                            <IconRun size={11} />
                                            <span className="tb-run-label">{group.shortLabel(task)}</span>
                                        </button>
                                    ))}
                                </div>
                            ))}
                            {otherRuns.length > 0 && (
                                <>
                                    <div className="tb-run-menu-head">
                                        <span>Running now</span>
                                    </div>
                                    {otherRuns.map((run) => (
                                        <button
                                            key={run.port}
                                            className="env-dd-item"
                                            role="menuitem"
                                            onClick={() => {
                                                close();
                                                if (run.owner.reveal) revealPortOwner(run.owner.reveal);
                                            }}>
                                            <span className="tb-run-state up" />
                                            <span className="tb-run-label">{run.process || "process"}</span>
                                            <span className="tb-run-menu-meta">:{run.port}</span>
                                            <span className={`tb-run-owner ${run.owner.kind}`}>
                                                {run.owner.kind === "agent" ? run.owner.label : "You"}
                                            </span>
                                            {run.owner.executionId && (
                                                <span
                                                    className="tb-run-stop"
                                                    role="button"
                                                    tabIndex={0}
                                                    aria-label={`Stop what listens on ${run.port}`}
                                                    onClick={(event) => {
                                                        event.stopPropagation();
                                                        stopRun(run.owner.terminalKey, run.owner.executionId!);
                                                    }}
                                                    onKeyDown={(event) => {
                                                        if (event.key !== "Enter" && event.key !== " ") return;
                                                        event.preventDefault();
                                                        event.stopPropagation();
                                                        stopRun(run.owner.terminalKey, run.owner.executionId!);
                                                    }}>
                                                    <IconStop size={9} />
                                                </span>
                                            )}
                                        </button>
                                    ))}
                                </>
                            )}
                        </div>
                    </>
                )}
            </span>
            <span className="tb-sep" />
        </>
    );
}
