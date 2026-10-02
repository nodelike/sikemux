import type { Agent, AgentBackendState } from "../state/types";
import type { ChatState } from "./types";

export function permissionModeOf(agent: Agent) {
    return agent.permissionMode ?? (agent.skipPermissions ? "bypass" : "workspace-write");
}

export function knownEffort(effort: string | undefined): Agent["effort"] {
    return ["off", "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"].includes(effort ?? "")
        ? (effort as Agent["effort"])
        : undefined;
}

export function backendState({
    connection,
    awaitingPermission,
    running,
}: {
    connection: ChatState["connection"];
    awaitingPermission: boolean;
    running: boolean;
}): AgentBackendState {
    return connection === "error" || connection === "stopped"
        ? "stopped"
        : awaitingPermission
          ? "blocked"
          : running
            ? "working"
            : connection === "ready"
              ? "idle"
              : "unknown";
}

export function connectingLabel(connection: ChatState["connection"]): string | null {
    if (connection === "installing") return "Installing structured-session adapter…";
    if (connection === "starting") return "Starting agent adapter…";
    if (connection === "connecting" || connection === "initializing") return "Connecting to agent session…";
    return null;
}

/* A permission card already says what the turn is waiting on, so a spinner
   beside it would only compete with it. */
export function activityText(
    state: Pick<ChatState, "permissions" | "running" | "messages" | "connection">,
    activeTool: string | null,
): string | null {
    return state.permissions.length > 0
        ? null
        : state.running
          ? (activeTool ?? "Thinking…")
          : state.messages.length > 0
            ? connectingLabel(state.connection)
            : null;
}

export function composerPlaceholder(
    state: Pick<ChatState, "connection" | "running">,
    { resuming, disconnected }: { resuming: boolean; disconnected: boolean },
): string {
    return state.connection === "ready"
        ? state.running
            ? "Send to queue behind the running turn"
            : "Ask about this project, or type / for commands"
        : resuming
          ? "Resuming — this message sends as soon as the session is back"
          : disconnected
            ? "Reconnect to continue this conversation"
            : state.connection === "installing"
              ? "Installing structured-session adapter…"
              : state.connection === "starting"
                ? "Starting agent adapter…"
                : "Connecting to agent session…";
}
