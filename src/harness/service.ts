import { invokeCommand } from "../api/invoke";
import { browserApi } from "../api/browser";
import { loadProjectConfig } from "../projectConfig";
import { trustProjectConfig } from "../projectConfigRuntime";
import { joinPath } from "../lib/paths";
import { collectPanes } from "../state/layout";
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
            if (previous)
                sessions[id] = { ...session, activeWindowId: previous.activeWindowId, activeAgentId: previous.activeAgentId, view: previous.view };
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

export const harnessTasks = new HarnessTasks(
    new NativeTaskExecutionBackend(),
    new WorkbenchTaskTerminalSurface(taskPtyBindings, (request) => preserveFocus(() => commands.openTaskTerminal(request))),
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
    if (request.agentId && !(state.agentsBySession[session.id] ?? []).includes(request.agentId))
        throw new Error("Agent does not belong to this project");
    return session;
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
            const existing = harnessTasks.existing(project, taskId, key);
            if (existing) return existing;
            const config = await loadProjectConfig(project);
            if (config.status !== "valid") throw new Error("Project needs a valid sikemux.json with tasks");
            const task = config.config.tasks.find((task) => task.id === taskId);
            if (!task) throw new Error("Task is not defined in sikemux.json");
            if (!(await trustProjectConfig(config))) throw new Error("Project configuration was not approved");
            const fresh = await loadProjectConfig(project);
            if (fresh.status !== "valid" || fresh.fingerprint !== config.fingerprint)
                throw new Error("Project configuration changed; inspect and retry");
            if (signal?.aborted) throw signal.reason;
            projectSession(request);
            const userTask = appTaskRuntime.getSnapshot(project);
            if (userTask?.task?.id === taskId && ["running", "stopping"].includes(userTask.status))
                throw new Error("This task is already running through the command deck");
            return harnessTasks.start(
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
            );
        }
        case "task.read": {
            const run = harnessTasks.get(project, text(params, "executionId")!);
            if (run.ptyId === undefined) return { ...run, output: "", cursor: 0, hasMore: false, truncated: false };
            const output = await invokeCommand<{ bytes: number[]; cursor: number; hasMore: boolean; truncated: boolean }>("harness_task_output", {
                id: run.ptyId,
                cursor: integer(params, "cursor", 0, Number.MAX_SAFE_INTEGER),
                limit: integer(params, "limit", 8192, 8192, 4),
            });
            return {
                ...run,
                output: new TextDecoder().decode(new Uint8Array(output.bytes)),
                cursor: output.cursor,
                hasMore: output.hasMore,
                truncated: output.truncated,
            };
        }
        case "task.stop":
            return harnessTasks.stop(project, text(params, "executionId")!);
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
                if (focus) {
                    commands.selectSession(session.id);
                    commands.selectAgent(request.agentId);
                }
                return { kind, tabId, url };
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
