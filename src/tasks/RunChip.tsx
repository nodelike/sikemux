import { useState } from "react";
import { openPortExternally, revealPortOwner } from "../ports/portActions";
import { taskTerminalReveal } from "../ports/projectPorts";
import { useProjectPorts } from "../ports/useProjectPorts";
import { SendToAgentMenu } from "../agents/SendToAgentMenu";
import { getState } from "../state/store";
import { reportError } from "../state/toast";
import { IconAgent, IconCheck, IconChevron, IconClose, IconExternal, IconPlus, IconRefresh, IconRun, IconStop, IconTerminal } from "../ui/Icons";
import { Tooltip } from "../ui/Tooltip";
import { tasksApi, useProjectTasks, useTaskRuns, type ProjectTask, type TaskRun } from "./coreTasks";
import { dismissPendingCheck, personRuns, runProjectTask, stopTaskRun, usePendingCheck, type PendingCheck } from "./projectTaskActions";
import { AddTaskDialog } from "./AddTaskDialog";
import { failureFixRequest, findTasksRequest, testFixRequest } from "./agentRequests";
import { FailureCard, TestResultsCard, type FinishedTestRun } from "./TestResultsCard";
import { useNow } from "../hooks/useNow";
import { basename } from "@shared/lib/paths";

function useSecondsSince(startedAt: number | null): number {
    const now = useNow(startedAt !== null);
    return startedAt === null ? 0 : Math.max(0, Math.floor((now - startedAt) / 1000));
}

function exitSummary(run: TaskRun): string {
    return run.exitCode !== undefined ? `exited ${run.exitCode}` : "failed";
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
    readonly tasks: readonly ProjectTask[];
    /** The label without the folder, since the group's heading names it. */
    readonly shortLabel: (task: ProjectTask) => string;
}

/** sikemux.json's tasks, then one group per folder the build-file tasks came from. */
function taskGroups(tasks: readonly ProjectTask[], project: string): TaskGroup[] {
    const groups: TaskGroup[] = [];
    const configured = tasks.filter((task) => task.source === "project" || task.source === "personal");
    if (configured.length) groups.push({ key: "config", title: "sikemux.json", meta: null, tasks: configured, shortLabel: (task) => task.label });
    const byFolder = new Map<string, ProjectTask[]>();
    for (const task of tasks.filter((candidate) => candidate.source === "detected" || candidate.source === "rerun"))
        byFolder.set(task.cwd, [...(byFolder.get(task.cwd) ?? []), task]);
    for (const [cwd, folderTasks] of byFolder) {
        const folder = cwd === "." ? basename(project) : cwd;
        const tool = BUILD_TOOL_LABELS[folderTasks[0].command.split(" ")[0]];
        const java = folderTasks[0].java;
        const javaText = java ? (java.runsOn !== null ? `Java ${java.runsOn}` : `needs Java ${java.required}`) : null;
        const suffix = cwd === "." ? "" : ` (${folder})`;
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

function CheckCard({ check }: { check: PendingCheck }) {
    const [fixing, setFixing] = useState(false);
    const installs = check.checks.some((item) => !item.ok && item.kind !== "port");
    const fixable = installs || check.checks.some((item) => !item.ok && item.kind === "port" && item.freePort !== null);
    const fix = () => {
        setFixing(true);
        void runProjectTask(check.project, check.task, { fix: true }).finally(() => setFixing(false));
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
                {check.checks.map((item) => (
                    <div key={item.kind} className={`tb-run-check-item${item.ok ? "" : " bad"}`}>
                        <span className="tb-run-check-mark">{item.ok ? <IconCheck size={11} /> : <IconClose size={11} />}</span>
                        <span className="tb-run-check-text">
                            <span>{item.title}</span>
                            <span className="tb-run-check-detail">{item.detail}</span>
                        </span>
                    </div>
                ))}
                <div className="tb-run-check-actions">
                    <button
                        className="tb-run-check-btn"
                        onClick={() => void runProjectTask(check.project, check.task, { force: true })}
                        disabled={fixing}>
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
    const [menuOpened, setMenuOpened] = useState(0);
    const offered = useProjectTasks(project, menuOpened);
    const allRuns = useTaskRuns();
    const ports = useProjectPorts(sessionId);
    const [picked, setPicked] = useState<Readonly<Record<string, string>>>({});
    const [open, setOpen] = useState(false);
    const pendingCheck = usePendingCheck();
    const [dismissedTests, setDismissedTests] = useState<string | null>(null);
    const [failureShown, setFailureShown] = useState<string | null>(null);
    const [adding, setAdding] = useState(false);
    const [agentPicker, setAgentPicker] = useState<{ x: number; y: number; text: string } | null>(null);
    const { running: runningRun, last } = personRuns(allRuns, project);
    const elapsed = useSecondsSince(runningRun?.startedAt ?? null);

    const tasks = offered?.tasks ?? [];
    const running = runningRun ? (tasks.find((task) => task.id === runningRun.taskId) ?? null) : null;
    const selected = running ?? tasks.find((task) => task.id === picked[project]) ?? tasks[0];
    if (!selected) return null;

    const key = runningRun?.terminalKey ?? (last?.taskId === selected.id ? last.terminalKey : null);
    const port = runningRun ? ports.find((candidate) => candidate.owner.executionId === runningRun.executionId) : undefined;
    const lastOfSelected = !running && last?.taskId === selected.id ? last : null;
    const failed = lastOfSelected?.status === "failed" ? exitSummary(lastOfSelected) : null;
    const java = selected.java;
    const url = port ? `http://localhost:${port.port}` : null;
    const otherRuns = ports.filter((candidate) => candidate.owner.kind !== "terminal" && candidate !== port);
    const testRun: FinishedTestRun | null =
        lastOfSelected?.tests && !runningRun ? { key: lastOfSelected.executionId, task: selected, results: lastOfSelected.tests } : null;
    const tests = testRun?.results ?? null;
    const testsCardOpen =
        testRun !== null && tests !== null && tests.failed > 0 && dismissedTests !== testRun.key && !open && pendingCheck?.project !== project;

    const close = () => setOpen(false);
    const showOutput = () => {
        const reveal = key ? taskTerminalReveal(getState(), sessionId, key) : null;
        if (reveal) revealPortOwner(reveal);
    };
    const primaryLabel = running ? `Stop ${running.label}` : `Run ${selected.label}`;
    const failureCardOpen = failed !== null && !tests && failureShown === (lastOfSelected?.executionId ?? null) && !open;
    const whyShown = (tests?.failed && dismissedTests === testRun?.key) || (failed !== null && !tests && !failureCardOpen);
    const status: { text: string; tone: string } | null = port
        ? { text: `:${port.port}`, tone: "" }
        : running
          ? { text: `${elapsed}s`, tone: "" }
          : tests
            ? tests.failed
                ? { text: `${tests.failed} failed`, tone: "failed" }
                : { text: `${tests.passed} passed`, tone: "passed" }
            : failed
              ? { text: failed, tone: "failed" }
              : java && java.runsOn === null
                ? { text: `needs Java ${java.required}`, tone: "warn" }
                : null;
    const pickAgent = (anchor: HTMLElement, text: string) => {
        const rect = anchor.getBoundingClientRect();
        setOpen(false);
        setAgentPicker({ x: rect.right, y: rect.bottom + 4, text });
    };
    const state = port ? "up" : running ? "running" : tests ? (tests.failed ? "failed" : "up") : failed ? "failed" : "idle";
    const rerunFailed = (run: FinishedTestRun) => {
        if (!lastOfSelected) return null;
        const executionId = lastOfSelected.executionId;
        return () => {
            setDismissedTests(run.key);
            void tasksApi
                .rerunFailed(executionId)
                .then(({ taskId }) => setPicked((current) => ({ ...current, [project]: taskId })))
                .catch(reportError("rerun failed tests"));
        };
    };
    return (
        <>
            <span className="tb-run" data-no-window-drag>
                <span className="tb-run-split">
                    <Tooltip label={primaryLabel}>
                        <button
                            className={`tb-run-btn${running ? " running" : ""}`}
                            onClick={() => (runningRun ? stopTaskRun(runningRun) : void runProjectTask(project, selected))}
                            aria-label={primaryLabel}>
                            {running ? <IconStop size={11} /> : <IconRun size={11} />}
                        </button>
                    </Tooltip>
                    <button
                        className="tb-run-chip"
                        onClick={() => {
                            if (!open) setMenuOpened((count) => count + 1);
                            setOpen((value) => !value);
                        }}
                        aria-haspopup="menu"
                        aria-expanded={open}
                        aria-label="Choose a task to run">
                        {state !== "idle" && <span className={`tb-run-state ${state}`} />}
                        <span className="tb-run-label">{selected.label}</span>
                        {status && <span className={`tb-run-meta ${status.tone}`}>· {status.text}</span>}
                        <IconChevron size={10} className="env-dd-chev" />
                    </button>
                </span>
                {whyShown && (
                    <Tooltip label={tests?.failed ? "Show the failed tests" : "Show why it stopped"}>
                        <button
                            className="tb-run-why"
                            onClick={() => (tests?.failed ? setDismissedTests(null) : setFailureShown(lastOfSelected?.executionId ?? null))}>
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
                        onFix={(anchor) => {
                            setDismissedTests(testRun.key);
                            pickAgent(anchor, testFixRequest(testRun, project));
                        }}
                    />
                )}
                {failureCardOpen && (
                    <FailureCard
                        task={selected}
                        summary={failed}
                        onClose={() => setFailureShown(null)}
                        onShowOutput={() => {
                            setFailureShown(null);
                            showOutput();
                        }}
                        onFix={(anchor) => {
                            setFailureShown(null);
                            pickAgent(anchor, failureFixRequest(selected, failed, project));
                        }}
                    />
                )}
                {agentPicker && (
                    <SendToAgentMenu
                        x={agentPicker.x}
                        y={agentPicker.y}
                        sessionId={sessionId}
                        alignRight
                        delivery={() => ({ text: agentPicker.text })}
                        onClose={() => setAgentPicker(null)}
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
                                            void runProjectTask(project, running);
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
                                                        void tasksApi.stop(run.owner.executionId!).catch(reportError("stop run"));
                                                    }}
                                                    onKeyDown={(event) => {
                                                        if (event.key !== "Enter" && event.key !== " ") return;
                                                        event.preventDefault();
                                                        event.stopPropagation();
                                                        void tasksApi.stop(run.owner.executionId!).catch(reportError("stop run"));
                                                    }}>
                                                    <IconStop size={9} />
                                                </span>
                                            )}
                                        </button>
                                    ))}
                                </>
                            )}
                            <div className="tb-run-menu-head">
                                <span>Your own tasks</span>
                            </div>
                            <button
                                className="env-dd-item"
                                role="menuitem"
                                onClick={() => {
                                    setOpen(false);
                                    setAdding(true);
                                }}>
                                <IconPlus size={12} />
                                <span>Add a task…</span>
                            </button>
                            <button
                                className="env-dd-item"
                                role="menuitem"
                                onClick={(event) => pickAgent(event.currentTarget, findTasksRequest(project))}>
                                <IconAgent size={12} />
                                <span>Find tasks with an agent…</span>
                            </button>
                        </div>
                    </>
                )}
            </span>
            {adding && (
                <AddTaskDialog
                    project={project}
                    onDone={(id) => {
                        setAdding(false);
                        if (id) setPicked((current) => ({ ...current, [project]: id }));
                    }}
                />
            )}
            <span className="tb-sep" />
        </>
    );
}
