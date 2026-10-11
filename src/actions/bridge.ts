import type { ActionContextInput } from "./registry";
import type { ApplicationActionMatch, ApplicationActionRuntime, ApplicationResolvedAction } from "./application";
import { activeAgentId } from "../state/selectors";
import type { StoreState } from "../state/store";
import type { LayoutNode, PaneNode } from "../state/types";

const EMPTY_ACTIONS: readonly ApplicationResolvedAction[] = Object.freeze([]);
const MAX_BRIDGE_SUBSCRIBERS = 128;
const listeners = new Set<() => void>();
let runtime: ApplicationActionRuntime | null = null;
let loadPromise: Promise<ApplicationActionRuntime> | null = null;
let revision = 0;

function publish(): void {
    revision += 1;
    for (const listener of Array.from(listeners)) {
        try {
            listener();
        } catch {
            listeners.delete(listener);
        }
    }
}

function install(loaded: ApplicationActionRuntime): ApplicationActionRuntime {
    if (runtime) return runtime;
    runtime = loaded;
    loaded.subscribe(publish);
    publish();
    return loaded;
}

export function loadApplicationActions(): Promise<ApplicationActionRuntime> {
    loadPromise ??= import("./application")
        .then(({ applicationActionRuntime }) => install(applicationActionRuntime))
        .catch((error: unknown) => {
            loadPromise = null;
            throw error;
        });
    return loadPromise;
}

export function subscribeApplicationActions(listener: () => void): () => void {
    if (typeof listener !== "function") throw new TypeError("application action subscriber must be a function");
    if (!listeners.has(listener) && listeners.size >= MAX_BRIDGE_SUBSCRIBERS) {
        throw new RangeError(`application action bridge cannot exceed ${MAX_BRIDGE_SUBSCRIBERS} subscribers`);
    }
    listeners.add(listener);
    return () => listeners.delete(listener);
}

export function getApplicationActionRevision(): number {
    return revision;
}

export function resolveApplicationActions(context: ActionContextInput): readonly ApplicationResolvedAction[] {
    return runtime?.resolve(context) ?? EMPTY_ACTIONS;
}

export function matchApplicationActionKeybinding(
    event: Pick<KeyboardEvent, "code" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey">,
    context: ActionContextInput,
): ApplicationActionMatch | null {
    return runtime?.matchKeybinding(event, context) ?? null;
}

export function executeApplicationAction(actionId: string, context: ActionContextInput): Promise<unknown> {
    if (!runtime) return Promise.reject(new Error("Application actions have not loaded"));
    return runtime.execute(actionId, context);
}

function findPane(node: LayoutNode, paneId: string): PaneNode | null {
    if (node.type === "pane") return node.id === paneId ? node : null;
    for (const child of node.children) {
        const pane = findPane(child, paneId);
        if (pane) return pane;
    }
    return null;
}

function actionFocus(target: EventTarget | null): { readonly target: string; readonly editable: boolean } | null {
    if (typeof Element === "undefined" || !(target instanceof Element)) return null;
    const terminal = target.closest(".xterm");
    const editable =
        !!terminal ||
        target.matches("input, textarea, select") ||
        !!target.closest('[contenteditable="true"], [contenteditable=""], [role="textbox"], .cm-content');
    return Object.freeze({
        target: terminal ? "terminal" : editable ? "text-input" : "application",
        editable,
    });
}

export function applicationActionContext(state: StoreState, focusTarget: EventTarget | null = null): ActionContextInput {
    const session = state.sessions[state.activeSessionId] ?? null;
    const window = session ? (state.windows[session.activeWindowId] ?? null) : null;
    const pane = window ? findPane(window.root, window.activePaneId) : null;
    const agentId = activeAgentId(state, session ?? undefined);
    const agent = agentId ? (state.agents[agentId] ?? null) : null;
    const activity = agent ? state.agentActivity[agent.id] : null;
    return Object.freeze({
        focusedItem: pane ? { id: pane.id, kind: pane.kind } : null,
        session: session ? { id: session.id, kind: session.kind } : null,
        project: session?.kind === "project" && session.cwd ? { id: session.id, root: session.cwd } : null,
        focus: actionFocus(focusTarget) ?? { target: pane ? "workbench-item" : "application", editable: false },
        modal: null,
        agent: agent ? { id: agent.id, kind: agent.type, status: activity?.state ?? "unknown" } : null,
        capabilities: session ? [`session.${session.kind}`, ...(pane ? [`item.${pane.kind}`] : [])] : [],
    });
}

/** Primitive selector used to avoid rebuilding context on unrelated store traffic. */
export function applicationActionContextFingerprint(state: StoreState): string {
    const session = state.sessions[state.activeSessionId];
    const window = session ? state.windows[session.activeWindowId] : undefined;
    const pane = window ? findPane(window.root, window.activePaneId) : null;
    const agentId = activeAgentId(state, session);
    const agent = agentId ? state.agents[agentId] : undefined;
    const activity = agent ? state.agentActivity[agent.id] : undefined;
    return JSON.stringify([session?.id, session?.kind, session?.cwd, window?.id, pane?.id, pane?.kind, agent?.id, agent?.type, activity?.state]);
}
