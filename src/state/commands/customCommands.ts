import { invokeCommand as invoke } from "../../api/invoke";
import { getState, mutate, setState } from "../store";
import { notify, reportError } from "../toast";
import { makePane, newId, replacePane, splitPane } from "../layout";
import { makeWindow } from "./shared";

export async function runBackgroundCommand(
    custom: import("../../commands/registry").CustomCommand,
    cwdOverride?: string,
    failOnNonZero = false,
    sessionIdOverride?: string,
): Promise<{ code: number; output: string }> {
    const st = getState();
    const session = st.sessions[sessionIdOverride ?? st.activeSessionId];
    if (!session) throw new Error("No active session for project command.");
    const commandCwd = cwdOverride || session.cwd;
    const result = await invoke<{ code: number; output: string }>("run_background_command", {
        command: custom.command,
        cwd: commandCwd || null,
        env: {
            SIKEMUX_SESSION_ID: session.id,
            SIKEMUX_SESSION_NAME: session.name,
            SIKEMUX_SESSION_KIND: session.kind,
            SIKEMUX_PROJECT: session.kind === "project" ? commandCwd : "",
        },
    });
    const summary = result.output.trim() || `exit ${result.code}`;
    notify(result.code === 0 ? "success" : "error", `${custom.title}: ${summary}`);
    if (failOnNonZero && result.code !== 0) throw new Error(`${custom.title} failed: ${summary}`);
    return result;
}

export function runCustomCommand(custom: import("../../commands/registry").CustomCommand, cwdOverride?: string): void {
    const st = getState();
    const session = st.sessions[st.activeSessionId];
    if (!session) return;
    const startup = custom.command;
    const commandCwd = cwdOverride || session.cwd;
    if (custom.placement === "background") {
        void runBackgroundCommand(custom, commandCwd).catch(reportError(custom.title));
        return;
    }
    if (custom.placement === "popup") {
        setState({
            commandPopup: {
                id: newId("popup"),
                title: custom.title,
                startup,
                cwd: commandCwd,
                context: {
                    sessionId: session.id,
                    sessionName: session.name,
                    sessionKind: session.kind,
                    ...(session.kind === "project" && commandCwd ? { project: commandCwd } : {}),
                },
            },
        });
        return;
    }
    mutate((d) => {
        const current = d.sessions[d.activeSessionId];
        if (!current) return;
        const window = d.windows[current.activeWindowId];
        if (!window) return;
        const pane = makePane(commandCwd, { startup });
        pane.title = custom.title;
        if (custom.placement === "terminal") {
            const ids = d.windowsBySession[current.id] ?? [];
            const created = makeWindow(commandCwd, custom.title, { startup });
            d.windows[created.id] = created;
            d.windowsBySession[current.id] = [...ids, created.id];
            current.activeWindowId = created.id;
        } else if (custom.placement === "split") {
            window.root = splitPane(window.root, window.activePaneId, "row", pane);
            window.activePaneId = pane.id;
        } else {
            window.root = replacePane(window.root, window.activePaneId, pane);
            window.activePaneId = pane.id;
        }
        d.zoomedPaneId = null;
    });
}

export function closeCommandPopup(): void {
    setState({ commandPopup: null });
}

export function upsertCustomCommand(command: import("../../commands/registry").CustomCommand): void {
    setState((s) => ({ customCommands: [...s.customCommands.filter((item) => item.id !== command.id), command] }));
}

export function deleteCustomCommand(id: string): void {
    setState((s) => ({ customCommands: s.customCommands.filter((item) => item.id !== id) }));
}

export function noteRecentCommand(key: string): void {
    setState((s) => ({ recentCommandKeys: [key, ...s.recentCommandKeys.filter((item) => item !== key)].slice(0, 20) }));
}
