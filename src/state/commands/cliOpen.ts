import { isPathWithin } from "../../lib/paths";
import { getState, mutate } from "../store";
import { collectPanes } from "../layout";
import type { CliOpenRequest, CliOpenResult, CliOpenTarget, Session } from "../types";
import { createProjectSession } from "./sessions";
import { makeWindow } from "./shared";

function cliProjectOwner(target: CliOpenTarget): Session | undefined {
    const st = getState();
    return st.sessionOrder
        .map((id) => st.sessions[id])
        .filter((session): session is Session => !!session && session.kind === "project" && isPathWithin(target.path, session.cwd))
        .sort((a, b) => b.cwd.length - a.cwd.length)[0];
}

function cliProjectRootOwner(projectRoot: string): Session | undefined {
    const st = getState();
    return st.sessionOrder
        .map((id) => st.sessions[id])
        .find((session): session is Session => !!session && session.kind === "project" && session.cwd === projectRoot);
}

/**
 * Focus the owning project for every CLI target and queue file targets for its
 * editor. Directory targets are complete as soon as their project is focused;
 * file targets are acknowledged by EditorPane only after the read succeeds.
 */
export function routeCliOpenRequest(request: CliOpenRequest): CliOpenResult[] {
    const immediate: CliOpenResult[] = [];

    for (const target of request.targets) {
        let owner = cliProjectOwner(target) ?? cliProjectRootOwner(target.projectRoot);
        if (!owner) {
            createProjectSession(target.projectRoot);
            owner = cliProjectOwner(target) ?? cliProjectRootOwner(target.projectRoot);
        }

        if (!owner) {
            immediate.push({
                requestId: request.id,
                targetId: target.id,
                paneId: null,
                path: target.path,
                error: `couldn't create a project session for ${target.projectRoot}`,
            });
            continue;
        }

        const ownerId = owner.id;
        if (target.kind === "directory") {
            mutate((d) => {
                const session = d.sessions[ownerId];
                if (!session) return;
                d.activeSessionId = ownerId;
                d.zoomedPaneId = null;
                d.pickerOpen = false;
                d.settingsOpen = false;
            });
            immediate.push({
                requestId: request.id,
                targetId: target.id,
                paneId: null,
                path: target.path,
                error: null,
            });
            continue;
        }

        // The editor is opened on demand, so a project that has never shown one
        // gets it created here rather than failing the request.
        if (!(getState().windowsBySession[ownerId] ?? []).some((id) => getState().windows[id]?.role === "files")) {
            mutate((d) => {
                const w = makeWindow(owner.cwd, "editor", { kind: "editor", role: "files" });
                d.windows[w.id] = w;
                d.windowsBySession[ownerId] = [...(d.windowsBySession[ownerId] ?? []), w.id];
            });
        }

        const st = getState();
        const fileWindowId = (st.windowsBySession[ownerId] ?? []).find((id) => st.windows[id]?.role === "files");
        const fileWindow = fileWindowId ? st.windows[fileWindowId] : undefined;
        const editorPane = fileWindow ? collectPanes(fileWindow.root).find((pane) => pane.kind === "editor") : undefined;
        if (!fileWindow || !editorPane) {
            immediate.push({
                requestId: request.id,
                targetId: target.id,
                paneId: null,
                path: target.path,
                error: `project ${owner.cwd} has no files editor`,
            });
            continue;
        }

        mutate((d) => {
            const session = d.sessions[ownerId];
            const win = d.windows[fileWindow.id];
            if (!session || !win) return;
            d.activeSessionId = ownerId;
            session.activeWindowId = win.id;
            win.activePaneId = editorPane.id;
            d.zoomedPaneId = null;
            d.pickerOpen = false;
            d.settingsOpen = false;
            const queued = d.pendingEditorOpens[editorPane.id] ?? [];
            if (!queued.some((item) => item.requestId === request.id && item.id === target.id)) {
                queued.push({ ...target, requestId: request.id });
            }
            d.pendingEditorOpens[editorPane.id] = queued;
        });
    }

    return immediate;
}

export function consumeCliEditorOpen(paneId: string, requestId: string, targetId: string): void {
    mutate((d) => {
        const queued = d.pendingEditorOpens[paneId];
        if (!queued) return;
        const next = queued.filter((item) => item.requestId !== requestId || item.id !== targetId);
        if (next.length === 0) delete d.pendingEditorOpens[paneId];
        else d.pendingEditorOpens[paneId] = next;
    });
}
