import type { ToolAction, ToolRow } from "../wire.generated";
import type { ChatRows } from "./rows";

/** What the agent is doing, as a verb and what it is on: the command it runs, the agent it waits for, why it retries. */
export interface Doing {
    verb: string;
    what?: string;
    /** `what` is a command, set as typed. */
    typed?: boolean;
}

const on = (verb: string, what: string | undefined, typed = false): Doing => (what?.trim() ? { verb, what, typed } : { verb });

/** What the line under the last row says the chat is doing, from what the core says it is doing. */
export function activityDoing(activity: ChatRows["activity"]): Doing | null {
    switch (activity?.kind) {
        case "retrying": {
            const verb = activity.connection ? "Reconnecting" : activity.attempt ? `Retrying, attempt ${activity.attempt}` : "Retrying";
            return on(verb, activity.reason);
        }
        case "call":
            return callDoing(activity.row);
        case "thinking":
            return { verb: "Thinking" };
        case "starting":
            return { verb: "Starting agent" };
        case "connecting":
            return { verb: "Connecting to agent session" };
        default:
            return null;
    }
}

/** The same, as one line of text, for a client that sets the verb and what it is on alike. */
export function activityText(activity: ChatRows["activity"]): string | null {
    const doing = activityDoing(activity);
    return doing && (doing.what ? `${doing.verb} ${doing.what}` : doing.verb);
}

const ACTION_DOING: Record<ToolAction, string> = {
    read: "Reading",
    edit: "Editing",
    delete: "Deleting",
    move: "Moving",
    search: "Searching",
    execute: "Running a command",
    think: "Thinking",
    fetch: "Fetching",
    switch_mode: "Switching mode",
    mcp: "Working",
    other: "Working",
};

/** What a running call is doing: its verb, and what it is on in its row's words. */
function callDoing(row: ToolRow): Doing {
    const target = row.target;
    const subject = row.subject ?? target;
    const agent = (name: string) => (!name || name.startsWith("@") ? name : `@${name}`);
    const { intent, family } = row;
    switch (intent) {
        case "run":
        case "test":
        case "build":
        case "git":
        case "commit":
            if (family === "shell") return on("Running", target, true);
            if (intent === "commit") return on("Committing", target);
            break;
        case "wait":
            return family === "waiting" && row.subject !== undefined ? on("Waiting for", agent(subject)) : on("Waiting", target);
        case "message":
            return on("Messaging", agent(subject));
        case "start":
            return family === "agents" ? on("Starting", agent(subject)) : on("Starting", subject);
        case "read":
            if (family === "files" || family === "shell") return on("Reading", subject);
            break;
        case "view":
            return on("Reading", subject);
        case "search":
            return family === "web" ? on("Searching the web for", subject) : on("Searching for", subject, true);
        case "list":
            return on("Listing", subject);
        case "edit":
            return on("Editing", target);
        case "write":
            return on("Writing", target);
        case "open":
            return on("Opening", target);
        case "click":
            return on(family === "simulator" ? "Tapping" : "Clicking", target);
        case "type":
            return on("Typing", target);
        case "look":
            return { verb: "Taking a screenshot" };
        case "fetch":
            return on("Reading", target);
    }
    return { verb: row.activity ?? ACTION_DOING[row.action] };
}

/** `commands` says whether the agent offers slash commands; a chat not started yet is taken to.
    `steers` says that sending while the turn runs goes into it.
    A message written while the agent starts goes out as soon as it is up, so the
    composer reads the same then as once it is ready. */
export function composerPlaceholder(
    state: Pick<ChatRows, "connection" | "running">,
    {
        resuming,
        disconnected,
        commands = true,
        steers = false,
        to,
    }: {
        resuming: boolean;
        disconnected: boolean;
        commands?: boolean;
        steers?: boolean;
        /** Who an idle chat's words go to, as the Mac says it in a project with a chief; `commands` adds the slash hint. */
        to?: { words: string; commands: boolean };
    },
): string {
    return resuming
        ? "Resuming — this message sends as soon as the session is back"
        : disconnected
          ? "Reconnect to continue this conversation"
          : state.running
            ? steers
                ? "Send to steer this turn"
                : "Send to queue behind the running turn"
            : to
              ? to.commands && commands
                  ? `${to.words}, or type / for commands`
                  : to.words
              : commands
                ? "Ask about this project, or type / for commands"
                : "Ask about this project";
}
