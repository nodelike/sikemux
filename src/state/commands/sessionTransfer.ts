import { getState, mutate, type StoreState } from "../store";
import { notify } from "../toast";
import { agentDirectCommand, agentStartup } from "./agentLaunchCommand";
import { parseSessionBundle } from "../sessionBundle";
import { agentIdsOf } from "../selectors";
import { agentWindow } from "../agentWindow";
import { copyText, readClipboardText } from "../../lib/clipboard";
import { collectPanes, cloneLayout, newId } from "../layout";
import type { Agent, Session, Window } from "../types";
import { attachSession } from "./shared";

function stripImportedStartup(node: Window["root"]): Window["root"] {
    if (node.type === "pane") return { ...node, startup: undefined, title: node.kind === "terminal" ? "shell" : node.title };
    return { ...node, children: node.children.map(stripImportedStartup) };
}

export async function exportActiveSession(): Promise<void> {
    const state = getState();
    const session = state.sessions[state.activeSessionId];
    if (!session) return;
    const windows = (state.windowsBySession[session.id] ?? []).map((id) => state.windows[id]).filter((w): w is Window => !!w && w.role !== "agent");
    const agents = agentIdsOf(state, session.id)
        .map((id) => state.agents[id])
        .filter((agent): agent is Agent => !!agent?.resumeId)
        .map(({ type, title, resumeId }) => ({ type, title, resumeId }));
    const payload = JSON.stringify({ format: "sikemux-session", version: 1, session, windows, agents }, (key, value) =>
        key === "secretVars" || key === "drafts" || key === "startup" || key === "baselineSessionIds" || key === "ptyId" ? undefined : value,
    );
    await copyText(payload);
    notify("success", `Copied ${session.name} session bundle (secrets and startup commands stripped)`);
}

export async function importSessionFromClipboard(): Promise<void> {
    const raw = await readClipboardText();
    // Parse and validate the complete untrusted payload before entering Immer.
    // Any error therefore leaves the store byte-for-byte unchanged.
    const bundle = parseSessionBundle(raw);
    const sourceName = bundle.session.name;
    const sourceCwd = bundle.session.cwd;
    const sourceKind = bundle.session.kind;
    mutate((d) => {
        const sessionId = newId("sess");
        const importedWindows: Window[] = [];
        for (const sourceWindow of bundle.windows) {
            const root = stripImportedStartup(cloneLayout(sourceWindow.root));
            const panes = collectPanes(root);
            const sourcePanes = collectPanes(sourceWindow.root);
            const sourceActiveIndex = sourcePanes.findIndex((pane) => pane.id === sourceWindow.activePaneId);
            importedWindows.push({
                ...sourceWindow,
                id: newId("win"),
                name: sourceWindow.name || "imported",
                root,
                activePaneId: panes[Math.max(0, sourceActiveIndex)].id,
                fixed: false,
            });
        }
        const session: Session = {
            id: sessionId,
            name: `${sourceName} imported`,
            kind: sourceKind,
            cwd: sourceCwd,
            pinned: false,
            activeWindowId: importedWindows[0].id,
        };
        attachSession(d as unknown as StoreState, session, importedWindows);
        for (const row of bundle.agents) {
            const id = newId("agent");
            d.agents[id] = {
                id,
                type: row.type,
                title: row.title,
                resumeId: row.resumeId,
                startup: agentStartup(row.type, row.resumeId),
                directCommand: agentDirectCommand(row.type, row.resumeId),
                launchState: "dormant",
            };
            const win = agentWindow(d.agents[id], sourceCwd);
            d.windows[win.id] = win;
            d.windowsBySession[sessionId].push(win.id);
        }
    });
    notify("success", "Imported session as a safe, dormant copy");
}
