import { activityLabel } from "./toolLabels";
import type { ChatRows } from "./types";

/** What the line under the last row says the chat is doing, from what the core says it is doing. */
export function activityText(activity: ChatRows["activity"]): string | null {
    switch (activity?.kind) {
        case "retrying":
            return `${activity.connection ? "Reconnecting…" : "Retrying…"}${activity.attempt ? ` (attempt ${activity.attempt})` : ""}`;
        case "call":
            return activityLabel(activity.row);
        case "thinking":
            return "Thinking…";
        case "starting":
            return "Starting agent…";
        case "connecting":
            return "Connecting to agent session…";
        default:
            return null;
    }
}

/** `commands` says whether the agent offers slash commands; a chat not started yet is taken to.
    `steers` says that sending while the turn runs goes into it.
    A message written while the agent starts goes out as soon as it is up, so the
    composer reads the same then as once it is ready. */
export function composerPlaceholder(
    state: Pick<ChatRows, "connection" | "running">,
    { resuming, disconnected, commands = true, steers = false }: { resuming: boolean; disconnected: boolean; commands?: boolean; steers?: boolean },
): string {
    return resuming
        ? "Resuming — this message sends as soon as the session is back"
        : disconnected
          ? "Reconnect to continue this conversation"
          : state.running
            ? steers
                ? "Send to steer this turn"
                : "Send to queue behind the running turn"
            : commands
              ? "Ask about this project, or type / for commands"
              : "Ask about this project";
}
