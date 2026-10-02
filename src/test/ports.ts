import type { ListeningPort, ListeningPortOwner } from "../api/ports";
import { getState, setState } from "../state/store";

export function terminalPort(port: number, owner: Partial<Extract<ListeningPortOwner, { kind: "pty" }>> = {}, process = "node"): ListeningPort {
    return {
        port,
        address: "127.0.0.1",
        pid: port,
        process,
        owner: { kind: "pty", ptyId: 1, project: null, paneId: null, agentId: null, taskExecutionId: null, ...owner },
    };
}

export function agentPort(port: number, agentId: string): ListeningPort {
    return { port, address: "0.0.0.0", pid: port, process: "next-server", owner: { kind: "agent", agentId } };
}

const agentWindow = (agentId: string) => ({
    id: `win-${agentId}`,
    name: agentId,
    role: "agent" as const,
    root: { type: "pane" as const, id: agentId, cwd: "/code", kind: "agent" as const, title: agentId },
    activePaneId: agentId,
});

/** One project with a shell, and a second project with an agent of its own. */
export function seedProjects(options: { terminal?: boolean; agentLaunch?: "live" | "dormant"; front?: "shell" | "agent-1" } = {}) {
    const { terminal = true, agentLaunch = "live", front = "shell" } = options;
    const shell = {
        id: "win-shell",
        name: "shell",
        role: "terminal" as const,
        root: { type: "pane" as const, id: "pane-1", cwd: "/code", kind: terminal ? ("terminal" as const) : ("editor" as const), title: "zsh" },
        activePaneId: "pane-1",
    };
    setState({
        sessions: {
            project: {
                id: "project",
                name: "project",
                kind: "project" as const,
                cwd: "/code",
                deploy: null,
                pinned: false,
                activeWindowId: front === "shell" ? "win-shell" : "win-agent-1",
            },
            other: {
                id: "other",
                name: "other",
                kind: "project" as const,
                cwd: "/other",
                deploy: null,
                pinned: false,
                activeWindowId: "win-agent-9",
            },
        },
        sessionOrder: ["project", "other"],
        activeSessionId: "project",
        windows: {
            "win-shell": shell,
            "win-agent-1": agentWindow("agent-1"),
            "win-agent-2": agentWindow("agent-2"),
            "win-agent-9": agentWindow("agent-9"),
        },
        windowsBySession: { project: ["win-shell", "win-agent-1", "win-agent-2"], other: ["win-agent-9"] },
        agents: {
            "agent-1": { id: "agent-1", type: "claude", title: "Claude", launchState: agentLaunch },
            "agent-2": { id: "agent-2", type: "codex", title: "Codex", launchState: agentLaunch },
            "agent-9": { id: "agent-9", type: "codex", title: "Elsewhere" },
        },
        agentActivity: {},
        terminalTitles: { "pane-1": "npm run dev" },
        desks: {},
        deskPanes: {},
    } as never);
    return getState();
}
