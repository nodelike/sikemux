import type { ListeningPort } from "../api/ports";
import { collectPanes } from "../state/layout";
import { agentIdsOf, activeAgentId, terminalLabel } from "../state/selectors";
import type { StoreState } from "../state/store";
import { taskPtyBindings, type TaskPtyBinding } from "../tasks/nativeRuntime";
import { terminalKeyOfExecution } from "../tasks/runtime";

type PortsState = Pick<StoreState, "sessions" | "windows" | "windowsBySession" | "agents" | "agentActivity" | "desks" | "terminalTitles">;

export type PortReveal =
    | { kind: "agent"; agentId: string }
    | { kind: "pane"; sessionId: string; windowId: string; paneId: string }
    | { kind: "desk-terminal"; agentId: string; id: string };

export interface ProjectPort {
    port: number;
    address: string;
    process: string;
    url: string;
    preview: boolean;
    owner: {
        kind: "terminal" | "task" | "agent";
        label: string;
        reveal: PortReveal | null;
        terminalKey?: string;
        /** Set when a task run listens here, so it can be stopped. */
        executionId?: string;
    };
}

const LOCAL_HOSTS = new Set(["localhost", "127.0.0.1", "0.0.0.0", "[::1]", "[::]"]);

/** The port a local preview URL points at, if it points at this machine. */
export function previewPort(url: string | undefined): number | null {
    if (!url) return null;
    try {
        const parsed = new URL(url);
        if (!LOCAL_HOSTS.has(parsed.hostname)) return null;
        if (parsed.port) return Number(parsed.port);
        return parsed.protocol === "https:" ? 443 : parsed.protocol === "http:" ? 80 : null;
    } catch {
        return null;
    }
}

function sessionPanes(state: PortsState, sessionId: string) {
    return (state.windowsBySession[sessionId] ?? []).flatMap((windowId) => {
        const win = state.windows[windowId];
        return win ? collectPanes(win.root).map((pane) => ({ windowId, pane })) : [];
    });
}

/** Something in the project could be serving: a terminal, or an agent that is running. */
export function hasLiveWork(state: PortsState, sessionId: string): boolean {
    if (sessionPanes(state, sessionId).some(({ pane }) => pane.kind === "terminal")) return true;
    return agentIdsOf(state, sessionId).some((id) => state.agents[id] && state.agents[id].launchState !== "dormant");
}

/** The running agent in front, or else the running one that worked last. Never one that is asleep. */
export function deskAgentFor(state: PortsState, sessionId: string): string | null {
    const live = agentIdsOf(state, sessionId).filter((id) => state.agents[id] && state.agents[id].launchState !== "dormant");
    const front = activeAgentId(state, state.sessions[sessionId]);
    if (front && live.includes(front)) return front;
    const lastWorked = (id: string) => state.agentActivity[id]?.updatedAt ?? 0;
    return live.sort((left, right) => lastWorked(right) - lastWorked(left))[0] ?? null;
}

function agentOwner(state: PortsState, agentId: string, executionId?: string): ProjectPort["owner"] {
    const terminalKey = executionId ? (terminalKeyOfExecution(executionId) ?? undefined) : undefined;
    return {
        kind: "agent",
        label: state.agents[agentId]?.title || "agent",
        reveal: { kind: "agent", agentId },
        ...(executionId ? { executionId, terminalKey } : {}),
    };
}

function taskTerminal(
    state: PortsState,
    sessionId: string,
    matches: (binding: TaskPtyBinding) => boolean,
): { label: string; reveal: PortReveal } | null {
    const bound = (id: string) => {
        const binding = taskPtyBindings.getSnapshot(id);
        return binding !== null && matches(binding);
    };
    for (const [agentId, desk] of Object.entries(state.desks))
        for (const terminal of desk.terminals)
            if (bound(terminal.id)) return { label: terminal.label, reveal: { kind: "desk-terminal", agentId, id: terminal.id } };
    const shown = sessionPanes(state, sessionId).find(({ pane }) => bound(pane.id));
    if (!shown) return null;
    return { label: shown.pane.title || "task", reveal: { kind: "pane", sessionId, windowId: shown.windowId, paneId: shown.pane.id } };
}

/** Where a task's output is showing, for opening it when the task fails. */
export function taskTerminalReveal(state: PortsState, sessionId: string, terminalKey: string): PortReveal | null {
    return taskTerminal(state, sessionId, (binding) => binding.terminalKey === terminalKey)?.reveal ?? null;
}

function taskOwner(state: PortsState, sessionId: string, executionId: string): ProjectPort["owner"] {
    const terminalKey = terminalKeyOfExecution(executionId) ?? undefined;
    const shown = taskTerminal(state, sessionId, (binding) => binding.executionId === executionId);
    return { kind: "task", label: shown?.label ?? "task", reveal: shown?.reveal ?? null, ...(terminalKey ? { terminalKey } : {}), executionId };
}

function ownerIn(state: PortsState, sessionId: string, project: string, port: ListeningPort): ProjectPort["owner"] | null {
    const owner = port.owner;
    if (owner.kind === "agent" || owner.agentId) {
        const agentId = owner.agentId!;
        const executionId = owner.kind === "pty" ? (owner.taskExecutionId ?? undefined) : undefined;
        return agentIdsOf(state, sessionId).includes(agentId) ? agentOwner(state, agentId, executionId) : null;
    }
    if (owner.taskExecutionId) return owner.project === project ? taskOwner(state, sessionId, owner.taskExecutionId) : null;
    if (owner.paneId) {
        const shown = sessionPanes(state, sessionId).find(({ pane }) => pane.id === owner.paneId);
        if (shown) {
            const label = terminalLabel(state.windows[shown.windowId], shown.pane, state.terminalTitles);
            return { kind: "terminal", label, reveal: { kind: "pane", sessionId, windowId: shown.windowId, paneId: shown.pane.id } };
        }
    }
    return owner.project === project ? { kind: "terminal", label: "terminal", reveal: null } : null;
}

/** The ports a project's own terminals, tasks and agents listen on, with its preview first. */
export function projectPorts(state: PortsState, sessionId: string, ports: readonly ListeningPort[], previewUrl?: string): ProjectPort[] {
    const project = state.sessions[sessionId]?.cwd;
    if (!project) return [];
    const preview = previewPort(previewUrl);
    return ports
        .flatMap((port) => {
            const owner = ownerIn(state, sessionId, project, port);
            if (!owner) return [];
            const isPreview = port.port === preview;
            return [
                {
                    port: port.port,
                    address: port.address,
                    process: port.process,
                    url: isPreview && previewUrl ? previewUrl : `http://localhost:${port.port}/`,
                    preview: isPreview,
                    owner,
                },
            ];
        })
        .sort((left, right) => Number(right.preview) - Number(left.preview) || left.port - right.port);
}
