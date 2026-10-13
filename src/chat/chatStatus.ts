import type { Agent, AgentBackendState } from "../state/types";
import type { ChatState } from "./types";

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
    if (connection === "starting") return "Starting agent…";
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

/** `commands` says whether the agent offers slash commands; a chat not started yet is taken to.
    A message written while the agent starts goes out as soon as it is up, so the
    composer reads the same then as once it is ready. */
export function composerPlaceholder(
    state: Pick<ChatState, "connection" | "running">,
    { resuming, disconnected, commands = true }: { resuming: boolean; disconnected: boolean; commands?: boolean },
): string {
    return resuming
        ? "Resuming — this message sends as soon as the session is back"
        : disconnected
          ? "Reconnect to continue this conversation"
          : state.running
            ? "Send to queue behind the running turn"
            : commands
              ? "Ask about this project, or type / for commands"
              : "Ask about this project";
}
