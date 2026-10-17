import { useEffect, useState, useSyncExternalStore } from "react";
import { openPortExternally, revealPortOwner } from "../ports/portActions";
import { taskTerminalReveal } from "../ports/projectPorts";
import { useProjectPorts } from "../ports/useProjectPorts";
import { projectControllerBridge } from "../projects/controllerBridge";
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
import type { TaskControllerSnapshot } from "./taskRegistry";

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

function CheckCard({ check }: { check: PendingCheck }) {
    const [fixing, setFixing] = useState(false);
    const fixable = check.items.some((item) => !item.ok && item.kind !== "port");
    const blockedByPort = check.items.some((item) => !item.ok && item.kind === "port");
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
                            {fixing ? "Installing…" : blockedByPort ? "Install" : "Install and run"}
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
                            {running && (
                                <button
                                    className="env-dd-item"
                                    role="menuitem"
                                    onClick={() => {
                                        close();
                                        restartProjectTask(project, config, running);
                                    }}>
                                    <IconRefresh size={12} />
                                    <span>Restart {running.label}</span>
                                </button>
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
                            {tasks.map((task) => {
                                const taskJava = javaChoiceOf(project, task.id);
                                return (
                                    <button
                                        key={task.id}
                                        className={`env-dd-item${task.id === selected.id ? " active" : ""}${task.source === "built-in" ? " detected" : ""}`}
                                        role="menuitemradio"
                                        aria-checked={task.id === selected.id}
                                        onClick={() => {
                                            close();
                                            setPicked((current) => ({ ...current, [project]: task.id }));
                                        }}>
                                        <IconRun size={11} />
                                        <span className="tb-run-label">{task.label}</span>
                                        {taskJava && (
                                            <span className={`tb-run-menu-meta${taskJava.runtime ? "" : " warn"}`}>
                                                {taskJava.runtime ? `Java ${taskJava.runtime.major}` : `needs Java ${taskJava.required}`}
                                            </span>
                                        )}
                                    </button>
                                );
                            })}
                        </div>
                    </>
                )}
            </span>
            <span className="tb-sep" />
        </>
    );
}
