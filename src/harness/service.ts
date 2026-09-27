import { invokeCommand } from "../api/invoke";
import { browserApi } from "../api/browser";
import { loadProjectConfig } from "../projectConfig";
import { trustProjectConfig } from "../projectConfigRuntime";
import { joinPath } from "../lib/paths";
import { collectPanes } from "../state/layout";
import { agentIdsOf } from "../state/selectors";
import { useStore, setState } from "../state/store";
import * as commands from "../state/commands";
import { appTaskRuntime } from "../tasks/application";
import { NativeTaskExecutionBackend, WorkbenchTaskTerminalSurface, taskPtyBindings } from "../tasks/nativeRuntime";
import { HarnessEvents } from "./events";
import { HarnessTasks } from "./tasks";

export interface HarnessRequest {
    id: string;
    project: string;
    agentId: string | null;
    method: string;
    params: Record<string, unknown>;
}

export const harnessEvents = new HarnessEvents();

function preserveFocus<T>(operation: () => T): T {
    const before = useStore.getState();
    try {
        return operation();
    } finally {
        const current = useStore.getState();
        const sessions = { ...current.sessions };
        for (const [id, session] of Object.entries(sessions)) {
            const previous = before.sessions[id];
            if (previous) sessions[id] = { ...session, activeWindowId: previous.activeWindowId };
        }
        setState({
            activeSessionId: before.activeSessionId,
            zoomedPaneId: before.zoomedPaneId,
            pickerOpen: before.pickerOpen,
            settingsOpen: before.settingsOpen,
            sessions,
        });
    }
}

/* A task an agent started goes on that agent's desk. One started with no agent
   behind it, or by one that has since closed, gets a tab in the workspace. */
export const harnessTasks = new HarnessTasks(
    new NativeTaskExecutionBackend(),
    new WorkbenchTaskTerminalSurface(taskPtyBindings, (request) =>
        request.agentId && useStore.getState().agents[request.agentId]
            ? commands.openDeskTerminal(request.agentId, request)
            : preserveFocus(() => commands.openTaskTerminal(request)),
    ),
    harnessEvents,
);

function text(params: Record<string, unknown>, key: string, required = true): string | undefined {
    const value = params[key];
    if (value === undefined && !required) return undefined;
    if (typeof value !== "string" || !value.trim() || value.length > 4096) throw new Error(`${key} must be nonempty text of at most 4096 characters`);
    return value;
}

function integer(params: Record<string, unknown>, key: string, fallback: number, max: number, min = 0): number {
    const value = params[key] ?? fallback;
    if (typeof value !== "number" || !Number.isSafeInteger(value) || value < min || value > max)
        throw new Error(`${key} must be an integer between ${min} and ${max}`);
    return value;
}

function projectSession(request: HarnessRequest) {
    const state = useStore.getState();
    const session = Object.values(state.sessions).find((session) => session.kind === "project" && session.cwd === request.project);
    if (!session) throw new Error("Project is not open in Sikemux");
    if (request.agentId && !agentIdsOf(state, session.id).includes(request.agentId)) throw new Error("Agent does not belong to this project");
    return session;
}

interface OutputQuery {
    cursor: number;
    limit: number;
    tail?: number;
    search?: string;
    context: number;
    plain: boolean;
}

interface OutputPage {
    bytes: number[];
    cursor: number;
    end: number;
    hasMore: boolean;
    truncated: boolean;
    matches?: number;
}

function readOutput(ptyId: number, query: OutputQuery): Promise<OutputPage> {
    return invokeCommand<OutputPage>("harness_task_output", { id: ptyId, query });
}

function executionFor(project: string, params: Record<string, unknown>): string {
    const executionId = text(params, "executionId", false);
    const taskId = text(params, "taskId", false);
    if (executionId && taskId) throw new Error("Pass either executionId or taskId, not both");
    if (executionId) return executionId;
    if (!taskId) throw new Error("executionId or taskId is required");
    const latest = harnessTasks.latest(project, taskId);
    if (!latest) throw new Error(`Task ${taskId} has not been started from this app session; start it with task_start`);
    return latest.executionId;
}

async function prepareLaunch(request: HarnessRequest, taskId: string, signal?: AbortSignal) {
    const { project } = request;
    const config = await loadProjectConfig(project);
    if (config.status === "absent") throw new Error("Project has no sikemux.json; add one that defines tasks");
    if (config.status === "invalid")
        throw new Error(`sikemux.json is invalid: ${config.errors.map((error) => `${error.path} ${error.message}`).join(" · ")}`);
    const task = config.config.tasks.find((task) => task.id === taskId);
    if (!task) throw new Error("Task is not defined in sikemux.json");
    if (!(await trustProjectConfig(config))) throw new Error("Project configuration was not approved");
    const fresh = await loadProjectConfig(project);
    if (fresh.status !== "valid" || fresh.fingerprint !== config.fingerprint) throw new Error("Project configuration changed; inspect and retry");
    if (signal?.aborted) throw signal.reason;
    projectSession(request);
    const userTask = appTaskRuntime.getSnapshot(project);
    if (userTask?.task?.id === taskId && ["running", "stopping"].includes(userTask.status))
        throw new Error("This task is already running through the command deck");
    return (key: string) =>
        harnessTasks.start(
            {
                taskId,
                project,
                source: "project",
                label: task.label,
                command: task.command,
                cwd: task.cwd === "." ? project : joinPath(project, task.cwd),
                env: task.env,
                cols: 120,
                rows: 30,
            },
            key,
            config.config.preview?.command === task.command ? config.config.preview.url : undefined,
            request.agentId ?? undefined,
        );
}

const READY_WAIT_MS = 45_000;

async function outputAppears(project: string, executionId: string, pattern: string, signal?: AbortSignal): Promise<boolean> {
    const deadline = Date.now() + READY_WAIT_MS;
    let cursor = harnessEvents.cursor;
    for (;;) {
        const run = harnessTasks.get(project, executionId);
        if (run.ptyId !== undefined) {
            const page = await readOutput(run.ptyId, { cursor: 0, limit: 4096, tail: 1, search: pattern, context: 0, plain: false });
            if (page.matches) return true;
        }
        const remaining = deadline - Date.now();
        if (!["starting", "running"].includes(run.status) || remaining <= 0) return false;
        cursor = (await harnessEvents.wait(project, cursor, Math.min(remaining, 30_000), executionId, signal)).cursor;
        await new Promise((resolve) => setTimeout(resolve, 250));
    }
}

export async function handleHarnessRequest(request: HarnessRequest, signal?: AbortSignal): Promise<unknown> {
    if (signal?.aborted) throw signal.reason;
    const session = projectSession(request);
    const { params, project } = request;
    switch (request.method) {
        case "workspace.inspect": {
            const state = useStore.getState();
            const config = await loadProjectConfig(project);
            return {
                project,
                sessionId: session.id,
                agentId: request.agentId,
                active: state.activeSessionId === session.id,
                activeWindowId: session.activeWindowId,
                windows: (state.windowsBySession[session.id] ?? [])
                    .map((id) => state.windows[id])
                    .filter(Boolean)
                    .map((window) => ({
                        id: window.id,
                        name: window.name,
                        role: window.role,
                        panes: collectPanes(window.root).map((pane) => ({
                            id: pane.id,
                            kind: pane.kind,
                            cwd: pane.cwd,
                            activePath: state.editorViews[pane.id]?.activePath,
                            openTabs: state.editorViews[pane.id]?.openTabs,
                        })),
                    })),
                tasks: config.status === "valid" ? config.config.tasks.map(({ id, label, command, cwd }) => ({ id, label, command, cwd })) : [],
                configStatus: config.status,
                configErrors: config.status === "invalid" ? config.errors : undefined,
                runs: harnessTasks.list(project),
                userTask: (() => {
                    const task = appTaskRuntime.getSnapshot(project);
                    return task ? { status: task.status, taskId: task.task?.id } : null;
                })(),
                cursor: harnessEvents.cursor,
            };
        }
        case "task.start": {
            const taskId = text(params, "taskId")!;
            const key = text(params, "idempotencyKey")!;
            if (key.length > 128) throw new Error("idempotencyKey must be at most 128 characters");
            const readyWhen = text(params, "readyWhen", false);
            const existing = harnessTasks.existing(project, taskId, key);
            const run = await (existing ?? (await prepareLaunch(request, taskId, signal))(key));
            return readyWhen ? { ...run, ready: await outputAppears(project, run.executionId, readyWhen, signal) } : run;
        }
        case "task.restart": {
            const taskId = text(params, "taskId")!;
            const launch = await prepareLaunch(request, taskId, signal);
            const previous = harnessTasks.latest(project, taskId);
            if (previous) await harnessTasks.stop(project, previous.executionId);
            return launch(crypto.randomUUID());
        }
        case "task.read": {
            const run = harnessTasks.get(project, executionFor(project, params));
            if (params.plain !== undefined && typeof params.plain !== "boolean") throw new Error("plain must be a boolean");
            const query: OutputQuery = {
                cursor: integer(params, "cursor", 0, Number.MAX_SAFE_INTEGER),
                limit: integer(params, "limit", 8192, 8192, 4),
                tail: params.tail === undefined ? undefined : integer(params, "tail", 1, 10_000, 1),
                search: text(params, "search", false),
                context: integer(params, "context", 3, 20),
                plain: params.plain === true,
            };
            if (run.ptyId === undefined) return { ...run, output: "", cursor: 0, end: 0, hasMore: false, truncated: false };
            const output = await readOutput(run.ptyId, query);
            return {
                ...run,
                output: new TextDecoder().decode(new Uint8Array(output.bytes)),
                cursor: output.cursor,
                end: output.end,
                hasMore: output.hasMore,
                truncated: output.truncated,
                matches: output.matches,
            };
        }
        case "task.stop":
            return harnessTasks.stop(project, executionFor(project, params));
        case "events.wait":
            return harnessEvents.wait(
                project,
                text(params, "cursor")!,
                integer(params, "timeoutMs", 30_000, 30_000),
                text(params, "executionId", false),
                signal,
            );
        case "ui.open": {
            const kind = text(params, "kind")!;
            if (params.focus !== undefined && typeof params.focus !== "boolean") throw new Error("focus must be a boolean");
            const focus = params.focus === true;
            if (kind === "preview") {
                if (!request.agentId) throw new Error("Opening a preview requires a Sikemux agent session");
                const config = await loadProjectConfig(project);
                const url = config.status === "valid" ? config.config.preview?.url : undefined;
                if (!url) throw new Error("No preview URL is configured in sikemux.json");
                const tabId = await browserApi.newTab(request.agentId, url);
                commands.showDeskBrowser(request.agentId);
                if (focus) {
                    commands.selectSession(session.id);
                    commands.selectAgent(request.agentId);
                }
                return { kind, tabId, url };
            }
            if (kind === "file" && request.agentId) {
                const agentId = request.agentId;
                const path = await invokeCommand<string>("harness_resolve_path", { project, path: text(params, "path")! });
                const line = integer(params, "line", 1, 10_000_000, 1) - 1;
                commands.openFileOnDesk(agentId, path, line, 0, { focus: false });
                if (focus) {
                    commands.selectSession(session.id);
                    commands.selectAgent(agentId);
                }
                harnessEvents.publish({ project, kind: "ui.opened" });
                return { kind, agentId, path };
            }
            const onDesk = kind === "terminal" ? commands.deskTerminalFor(harnessTasks.get(project, text(params, "executionId")!).executionId) : null;
            if (onDesk) {
                commands.showDeskTerminal(onDesk.agentId, onDesk.id);
                if (focus) {
                    commands.selectSession(session.id);
                    commands.selectAgent(onDesk.agentId);
                }
                harnessEvents.publish({ project, kind: "ui.opened" });
                return { kind, agentId: onDesk.agentId };
            }
            const path = kind === "file" ? await invokeCommand<string>("harness_resolve_path", { project, path: text(params, "path")! }) : undefined;
            const line = integer(params, "line", 1, 10_000_000, 1) - 1;
            const open = () => {
                commands.selectSession(session.id);
                if (kind === "file" && !focus) {
                    commands.openEditorPane();
                    const state = useStore.getState();
                    const window = state.windows[state.sessions[session.id].activeWindowId];
                    const pane = collectPanes(window.root).find((pane) => pane.kind === "editor");
                    if (!pane) throw new Error("Project has no editor pane");
                    commands.openEditorTab(pane.id, path!, false);
                } else if (kind === "file") {
                    const failures = commands.routeCliOpenRequest({
                        id: request.id,
                        cwd: project,
                        wait: false,
                        targets: [{ id: request.id, kind: "file", path: path!, projectRoot: project, line }],
                    });
                    if (failures.some((result) => result.error)) throw new Error(failures.find((result) => result.error)!.error!);
                } else if (kind === "diff") commands.openDiffPane();
                else if (kind === "terminal") {
                    const run = harnessTasks.get(project, text(params, "executionId")!);
                    const window = (useStore.getState().windowsBySession[session.id] ?? [])
                        .map((id) => useStore.getState().windows[id])
                        .find((window) =>
                            collectPanes(window.root).some((pane) => taskPtyBindings.getSnapshot(pane.id)?.executionId === run.executionId),
                        );
                    if (!window) throw new Error("Task terminal is no longer open");
                    commands.selectWindowId(window.id);
                } else throw new Error("kind must be file, diff, terminal, or preview");
                return { kind, windowId: useStore.getState().sessions[session.id].activeWindowId, path };
            };
            const result = focus ? open() : preserveFocus(open);
            harnessEvents.publish({ project, kind: "ui.opened" });
            return result;
        }
        default:
            throw new Error("Unknown harness method");
    }
}
