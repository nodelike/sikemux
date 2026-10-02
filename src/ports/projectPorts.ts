import type { ListeningPort } from "../api/ports";
import { collectPanes } from "../state/layout";
import { agentIdsOf, activeAgentId } from "../state/selectors";
import type { StoreState } from "../state/store";
import { taskPtyBindings } from "../tasks/nativeRuntime";

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
    owner: { kind: "terminal" | "task" | "agent"; label: string; reveal: PortReveal | null };
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

function agentOwner(state: PortsState, agentId: string): ProjectPort["owner"] {
    return { kind: "agent", label: state.agents[agentId]?.title || "agent", reveal: { kind: "agent", agentId } };
}

function taskOwner(state: PortsState, sessionId: string, executionId: string): ProjectPort["owner"] {
    for (const [agentId, desk] of Object.entries(state.desks))
        for (const terminal of desk.terminals)
            if (taskPtyBindings.getSnapshot(terminal.id)?.executionId === executionId)
                return { kind: "task", label: terminal.label, reveal: { kind: "desk-terminal", agentId, id: terminal.id } };
    const shown = sessionPanes(state, sessionId).find(({ pane }) => taskPtyBindings.getSnapshot(pane.id)?.executionId === executionId);
    if (shown)
        return {
            kind: "task",
            label: shown.pane.title || "task",
            reveal: { kind: "pane", sessionId, windowId: shown.windowId, paneId: shown.pane.id },
        };
    return { kind: "task", label: "task", reveal: null };
}

function ownerIn(state: PortsState, sessionId: string, project: string, port: ListeningPort): ProjectPort["owner"] | null {
    const owner = port.owner;
    if (owner.kind === "agent" || owner.agentId) {
        const agentId = owner.agentId!;
        return agentIdsOf(state, sessionId).includes(agentId) ? agentOwner(state, agentId) : null;
    }
    if (owner.taskExecutionId) return owner.project === project ? taskOwner(state, sessionId, owner.taskExecutionId) : null;
    if (owner.paneId) {
        const shown = sessionPanes(state, sessionId).find(({ pane }) => pane.id === owner.paneId);
        if (shown) {
            const label = state.terminalTitles[shown.pane.id] || shown.pane.title || "terminal";
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
