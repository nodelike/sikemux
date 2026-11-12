import type { ToolAction, ToolRow } from "../wire.generated";
import type { ChatRows } from "./rows";
import { english, type Say } from "../lib/say";

/**
 * What the agent is doing, as a verb and what it is on: the command it runs, the agent it waits for, why it retries.
 * A language puts the verb before or after what it is on, so the words come from a whole sentence with `{what}` in it.
 */
export interface Doing {
    verb: string;
    what?: string;
    /** The words after `what`, in a language that says them there. */
    after?: string;
    /** `what` is a command, set as typed. */
    typed?: boolean;
}

function on(sentence: string, what: string, typed = false): Doing {
    const [before, after = ""] = sentence.split("{what}");
    return { verb: before.trim(), what, ...(after.trim() ? { after: after.trim() } : {}), typed };
}

/** What the line under the last row says the chat is doing, from what the core says it is doing. */
export function activityDoing(activity: ChatRows["activity"], say: Say = english): Doing | null {
    switch (activity?.kind) {
        case "retrying": {
            const why = activity.reason?.trim();
            if (!why) {
                const verb = activity.connection
                    ? say("Reconnecting")
                    : activity.attempt
                      ? say("Retrying, attempt {attempt}", { attempt: activity.attempt })
                      : say("Retrying");
                return { verb };
            }
            const sentence = activity.connection
                ? say("Reconnecting: {what}")
                : activity.attempt
                  ? say("Retrying, attempt {attempt}: {what}", { attempt: activity.attempt })
                  : say("Retrying: {what}");
            return on(sentence, why);
        }
        case "call":
            return callDoing(activity.row, say);
        case "thinking":
            return { verb: say("Thinking") };
        case "starting":
            return { verb: say("Starting agent") };
        case "connecting":
            return { verb: say("Connecting to agent session") };
        default:
            return null;
    }
}

/** The same, as one line of text, for a client that sets the verb and what it is on alike. */
export function activityText(activity: ChatRows["activity"], say: Say = english): string | null {
    const doing = activityDoing(activity, say);
    return doing && [doing.verb, doing.what, doing.after].filter(Boolean).join(" ");
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
function callDoing(row: ToolRow, say: Say): Doing {
    const target = row.target;
    const subject = row.subject ?? target;
    if (!subject.trim()) return { verb: row.activity ?? say(ACTION_DOING[row.action]) };
    const agent = (name: string) => (!name || name.startsWith("@") ? name : `@${name}`);
    const { intent, family } = row;
    switch (intent) {
        case "run":
        case "test":
        case "build":
        case "git":
        case "commit":
            if (family === "shell") return on(say("Running {what}"), target, true);
            if (intent === "commit") return on(say("Committing {what}"), target);
            break;
        case "wait":
            return family === "waiting" && row.subject !== undefined
                ? on(say("Waiting for {what}"), agent(subject))
                : on(say("Waiting {what}"), target);
        case "message":
            return on(say("Messaging {what}"), agent(subject));
        case "start":
            return family === "agents" ? on(say("Starting {what}"), agent(subject)) : on(say("Starting {what}"), subject);
        case "read":
            if (family === "files" || family === "shell") return on(say("Reading {what}"), subject);
            break;
        case "view":
            return on(say("Reading {what}"), subject);
        case "search":
            return family === "web" ? on(say("Searching the web for {what}"), subject) : on(say("Searching for {what}"), subject, true);
        case "list":
            return on(say("Listing {what}"), subject);
        case "edit":
            return on(say("Editing {what}"), target);
        case "write":
            return on(say("Writing {what}"), target);
        case "open":
            return on(say("Opening {what}"), target);
        case "click":
            return on(say(family === "simulator" ? "Tapping {what}" : "Clicking {what}"), target);
        case "type":
            return on(say("Typing {what}"), target);
        case "look":
            return { verb: say("Taking a screenshot") };
        case "fetch":
            return on(say("Reading {what}"), target);
    }
    return { verb: row.activity ?? say(ACTION_DOING[row.action]) };
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
    say: Say = english,
): string {
    return resuming
        ? say("Resuming — this message sends as soon as the session is back")
        : disconnected
          ? say("Reconnect to continue this conversation")
          : state.running
            ? steers
                ? say("Send to steer this turn")
                : say("Send to queue behind the running turn")
            : to
              ? to.commands && commands
                  ? say("{words}, or type / for commands", { words: to.words })
                  : to.words
              : commands
                ? say("Ask about this project, or type / for commands")
                : say("Ask about this project");
}
