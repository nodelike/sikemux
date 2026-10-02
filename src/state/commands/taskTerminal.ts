import { isPathWithin } from "../../lib/paths";
import type { TaskTerminalPresentationRequest } from "../../tasks/nativeRuntime";
import { mutate } from "../store";
import { collectPanes } from "../layout";
import { makeWindow } from "./shared";

function taskTerminalContainsControl(value: string): boolean {
    for (let index = 0; index < value.length; index += 1) {
        const code = value.charCodeAt(index);
        if (code <= 31 || (code >= 127 && code <= 159)) return true;
    }
    return false;
}

function requireTaskTerminalText(name: string, value: string, maxLength: number): string {
    if (
        typeof value !== "string" ||
        value.length === 0 ||
        value.length > maxLength ||
        value.trim().length === 0 ||
        taskTerminalContainsControl(value)
    ) {
        throw new TypeError(`${name} must be bounded non-blank text without control characters`);
    }
    return value;
}

/**
 * Open or focus a transient task-output window and return its pane ID. PTY IDs,
 * commands, and environment values are intentionally absent from this state.
 */
export function openTaskTerminal(request: TaskTerminalPresentationRequest): string {
    const project = requireTaskTerminalText("task project", request.project, 4_096);
    const cwd = requireTaskTerminalText("task cwd", request.cwd, 4_096);
    const terminalKey = requireTaskTerminalText("task terminal key", request.terminalKey, 8_192);
    const label = requireTaskTerminalText("task label", request.label, 256);
    if (!isPathWithin(cwd, project)) throw new TypeError("task working directory must stay within its project");
    if (request.signal.aborted) throw request.signal.reason ?? new DOMException("Task terminal presentation was aborted", "AbortError");

    let paneId: string | null = null;
    mutate((d) => {
        const owner = d.sessionOrder.map((id) => d.sessions[id]).find((session) => session?.kind === "project" && session.cwd === project);
        if (!owner) return;
        const windowIds = d.windowsBySession[owner.id] ?? [];
        for (const windowId of windowIds) {
            const candidate = d.windows[windowId];
            if (!candidate?.transient) continue;
            const pane = collectPanes(candidate.root).find((item) => item.externalPty && item.taskTerminalKey === terminalKey);
            if (!pane) continue;
            pane.cwd = cwd;
            pane.title = label;
            candidate.name = label;
            candidate.activePaneId = pane.id;
            paneId = pane.id;
            if (request.background) return;
            owner.activeWindowId = candidate.id;
            d.activeSessionId = owner.id;
            d.zoomedPaneId = null;
            return;
        }

        const created = makeWindow(cwd, label, { role: "term" });
        created.transient = true;
        if (created.root.type !== "pane") return;
        created.root.title = label;
        created.root.externalPty = true;
        created.root.taskTerminalKey = terminalKey;
        d.windows[created.id] = created;
        d.windowsBySession[owner.id] = [...windowIds, created.id];
        paneId = created.root.id;
        if (request.background) return;
        owner.activeWindowId = created.id;
        d.activeSessionId = owner.id;
        d.zoomedPaneId = null;
    });
    if (!paneId) throw new Error("Task project is no longer open");
    return paneId;
}
