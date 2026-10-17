import { useEffect, useState, useSyncExternalStore } from "react";
import { openPortExternally, revealPortOwner } from "../ports/portActions";
import { taskTerminalReveal } from "../ports/projectPorts";
import { useProjectPorts } from "../ports/useProjectPorts";
import { projectControllerBridge } from "../projects/controllerBridge";
import { getState } from "../state/store";
import { IconChevron, IconExternal, IconRefresh, IconRun, IconStop, IconTerminal } from "../ui/Icons";
import { Tooltip } from "../ui/Tooltip";
import { appTaskRuntime, getAppTaskSnapshot, javaChoiceOf, subscribeAppTasks, subscribeTaskRuntime } from "./application";
import { projectTasksFor, restartProjectTask, runProjectTask, stopProjectTask } from "./projectTaskActions";
import { taskTerminalKey } from "./runtime";
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

export function RunChip({ sessionId, project }: { sessionId: string; project: string }) {
    const registry = useSyncExternalStore(subscribeAppTasks, getAppTaskSnapshot);
    const controller = useSyncExternalStore(projectControllerBridge.subscribe, projectControllerBridge.getActiveSnapshot);
    const runtime = useSyncExternalStore(subscribeTaskRuntime, () => appTaskRuntime.getSnapshot(project));
    const ports = useProjectPorts(sessionId);
    const [picked, setPicked] = useState<Readonly<Record<string, string>>>({});
    const [open, setOpen] = useState(false);
    const elapsed = useSecondsSince(runtime?.activeRunId ?? null);

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

    const close = () => setOpen(false);
    const showOutput = () => {
        const reveal = taskTerminalReveal(getState(), sessionId, key);
        if (reveal) revealPortOwner(reveal);
    };
    const primaryLabel = running ? `Stop ${running.label}` : `Run ${selected.label}`;
    const state = port ? "up" : running ? "running" : failed ? "failed" : "idle";
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
                    {state === "failed" && <span className="tb-run-meta failed">{failed}</span>}
                    {java && (
                        <span className={`tb-run-meta${java.runtime ? "" : " warn"}`}>
                            {java.runtime ? `Java ${java.runtime.major}` : `needs Java ${java.required}`}
                        </span>
                    )}
                    <IconChevron size={10} className="env-dd-chev" />
                </button>
                {failed && (
                    <Tooltip label="Show the output">
                        <button className="tb-run-why" onClick={showOutput}>
                            Why?
                        </button>
                    </Tooltip>
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
