import type { PersonCall, TurnPhase, TurnState, TurnStep, TurnWait, WorkAt } from "../wire.generated";
import { clockLabel } from "../chat/messageMeta";

/**
 * Every word a phone says about how a turn stands and about a call, in one table, as the Mac's
 * `app/src/turn_line.rs` and `app/src/calls.rs` say them. The iPhone's `Calls/CallWords.swift` holds the same words.
 */

const MINUTE = 60_000;

// The turn's line

/** The state's word, then what it is on and how long, said between dots. */
export type Said = { word: string; phase: TurnPhase; parts: string[] };

/**
 * What `turn` says for an agent of `maker` (its own name, such as Claude). Working with nothing said about what it is
 * on says nothing, so the chat shows its live activity instead.
 */
export function said(turn: TurnState, maker: string, now = Date.now()): Said | undefined {
    const minutes = (from: number) => Math.max(1, Math.floor((now - from) / MINUTE));
    switch (turn.state) {
        case "working":
            if (!turn.on) return undefined;
            return { word: "Working", phase: "working", parts: [lowered(turn.on), elapsed(Math.floor((now - turn.since) / 1000))] };
        case "waiting": {
            if (!turn.wait) return undefined;
            const gone = waited(turn.wait, now);
            return { word: waitingOn(turn.wait), phase: "waiting", parts: gone ? [gone] : [] };
        }
        case "needsYou": {
            const call = turn.call;
            if (!call) return undefined;
            const more = call.count > 1 ? ` and ${call.count - 1} more` : "";
            return { word: "Needs you", phase: "needsYou", parts: [`${call.question}${more}`, `${minutes(call.waitingSince)} min`] };
        }
        case "done": {
            const done = turn.done;
            if (!done) return { word: "Done", phase: "done", parts: [] };
            const parts = [changed(done.changes)];
            if (done.changes !== "none") {
                if (done.checks === "pass") parts.push(done.tests != null ? `${done.tests} tests pass` : "checks pass");
                else if (done.checks === "failed") parts.push("checks fail");
            }
            return { word: "Done", phase: "done", parts };
        }
        case "stopped": {
            const parts = turn.stopped?.left ? [lowered(turn.stopped.left)] : [];
            parts.push("nothing set to wake it");
            return { word: "Stopped", phase: "stopped", parts };
        }
        case "failed": {
            const failed = turn.failed;
            if (!failed) return undefined;
            const why =
                failed.why === "limit"
                    ? `${maker}'s limit`
                    : failed.why === "signIn"
                      ? `signed out of ${maker}`
                      : failed.why === "network"
                        ? `lost the connection to ${maker}`
                        : (failed.said ?? "the turn did not finish");
            const parts = [why];
            if (failed.resetsAt != null && failed.resetsAt > now) parts.push(`resets ${clockLabel(failed.resetsAt, now)}`);
            return { word: "Failed", phase: "failed", parts };
        }
        default:
            return undefined;
    }
}

/**
 * The line over a call docked on the chat, which already shows the question: the word, how many calls wait when more
 * than one does, and how long.
 */
export function docked(line: Said, waiting: number): Said {
    if (line.phase !== "needsYou") return line;
    return { ...line, parts: [...(waiting > 1 ? [`${waiting} calls`] : []), ...line.parts.slice(-1)] };
}

export function waitingOn(wait: TurnWait): string {
    switch (wait.kind) {
        case "ci":
            return "Waiting on CI";
        case "deploy":
            return "Waiting on the deploy";
        case "check":
            return "Waiting on the checks";
        case "agent":
            return wait.what.startsWith("@") ? `Waiting on ${wait.what}` : `Waiting on @${wait.what}`;
        case "timer":
            return wait.until != null ? `Waiting until ${clockLabel(wait.until)}` : "Waiting";
        default:
            return `Waiting on ${wait.what}`;
    }
}

/** How long a wait has gone, against how long it usually takes once that is known. */
export function waited(wait: TurnWait, now = Date.now()): string | undefined {
    if (wait.kind === "timer") return undefined;
    const gone = Math.floor(Math.max(0, now - wait.startedAt) / MINUTE);
    if (wait.expectedMs == null) return `${gone} min`;
    return `${gone} of ~${Math.ceil(wait.expectedMs / MINUTE)} min`;
}

export function changed(changes: WorkAt): string {
    switch (changes) {
        case "uncommitted":
            return "uncommitted";
        case "committed":
            return "committed";
        case "landed":
            return "landed";
        case "staged":
            return "on staging";
        case "shipped":
            return "shipped";
        default:
            return "no changes";
    }
}

/** The words on a step's button. */
export function stepLabel(step: TurnStep): string | undefined {
    switch (step) {
        case "commit":
            return "Commit";
        case "push":
            return "Push";
        case "deployStaging":
            return "Deploy to staging";
        case "shipProduction":
            return "Ship to production…";
        case "rollBack":
            return "Roll back";
        case "tryIt":
            return "Try it";
        case "carryOn":
            return "Carry on";
        case "undo":
            return "Undo";
        default:
            return undefined;
    }
}

/** The button that brings the call a waiting agent asks into view. */
export const LOOK = "Look";

/** How long a turn has run, clock style as the Mac says it: `45s`, `3m 07s`, `2h 05m`. */
export function elapsed(seconds: number): string {
    const s = Math.max(0, seconds);
    const two = (value: number) => String(value).padStart(2, "0");
    if (s < 60) return `${s}s`;
    if (s < 3600) return `${Math.floor(s / 60)}m ${two(s % 60)}s`;
    return `${Math.floor(s / 3600)}h ${two(Math.floor(s / 60) % 60)}m`;
}

/** The agent's own words, as they read after a dot: the first letter small unless it starts a name. */
export function lowered(words: string): string {
    let trimmed = words.trim();
    while (trimmed.endsWith(".")) trimmed = trimmed.slice(0, -1);
    if (trimmed.length < 2) return trimmed;
    const second = trimmed[1];
    return second === second.toUpperCase() && second !== second.toLowerCase() ? trimmed : trimmed[0].toLowerCase() + trimmed.slice(1);
}

// A row's short mark

/**
 * How far a CI or check wait with an estimate has gone, which its row draws as a filling ring; undefined for any other
 * wait, which the row draws as the pink timer.
 */
export function ringShare(wait: TurnWait | undefined, now = Date.now()): number | undefined {
    if (!wait || (wait.kind !== "ci" && wait.kind !== "check") || !wait.expectedMs) return undefined;
    return Math.min(1, Math.max(0, (now - wait.startedAt) / wait.expectedMs));
}

/**
 * The time beside a row's mark: how long a call has waited, the minutes left on a wait with an estimate, or how long
 * until a set time; nothing else has a time.
 */
export function rowMinutes(turn: TurnState, now = Date.now()): string | undefined {
    if (turn.state === "needsYou") {
        return turn.call ? `${Math.max(1, Math.floor((now - turn.call.waitingSince) / MINUTE))}m` : undefined;
    }
    if (turn.state !== "waiting" || !turn.wait) return undefined;
    const wait = turn.wait;
    if (ringShare(wait, now) != null && wait.expectedMs) {
        return `${Math.max(1, Math.ceil((wait.startedAt + wait.expectedMs - now) / MINUTE))}m`;
    }
    return wait.until != null ? countdown(wait.until - now) : undefined;
}

/** Time left until something fires: now, 12m, 3h, 4d; minutes rounded up under an hour. */
export function countdown(ms: number): string {
    if (ms <= 0) return "now";
    const minutes = Math.ceil(ms / MINUTE);
    if (minutes < 60) return `${minutes}m`;
    if (minutes < 24 * 60) return `${Math.floor(minutes / 60)}h`;
    return `${Math.floor(minutes / (24 * 60))}d`;
}

/** What a row's mark says to a screen reader, the hover card's words on the Mac. */
export function rowLabel(turn: TurnState, now = Date.now()): string {
    const time = rowMinutes(turn, now) ?? "";
    switch (turn.state) {
        case "working":
            return "Working";
        case "waiting":
            return turn.wait ? `${waitingOn(turn.wait)}, ${time}` : "Waiting";
        case "needsYou":
            return `Needs you, ${time}`;
        case "done":
            return "Done";
        case "stopped":
            return "Stopped";
        case "failed":
            return "Failed";
        default:
            return "";
    }
}

// The chief

/** The chief's row: what follows it is its chat's own title, and where projects share the list, the project. */
export const CHIEF = "@Chief";
/** The composer's words in the chief's own chat. */
export const TELL_CHIEF = "Tell @Chief what you want";
/** The composer's words in a worker's chat, in a project with a chief. */
export const sendTo = (name: string) => `Send to @${name}`;
/** Under the chief's row when a tap cannot reach the host to open its chat. */
export const CHIEF_OFFLINE = "Not connected to this host yet, so its chief can't open.";

// An assumption

/** After the agent's name: what it went on assuming. */
export const assumed = (assumption: string) => `assumed ${assumption}`;
export const FINE = "Fine";
export const CHANGE = "Change";
export const couldNotRead = (problem: string) => `could not read this agent's questions: ${problem}`;

// The call card

export const PICK = "pick";
export const AGAINST_IT = "Against it:";
export const ADD_NOTE = "Add a note…";
export const OWN_WORDS = "Or answer in your own words…";
export const SECRET_FIELD = "Type or paste it…";
export const GO_WITH_THIS = "Go with this";
export const NOT_MINE = "Not mine";
export const WHAT_RUNS = "What runs";
export const kept = (name: string) => `Kept private: @${name} reads it, and it never shows in the chat.`;
/** Go with this while the host has not yet taken that this phone drew the call. */
export const CONNECTING = "Connecting…";
/** Which of an agent's waiting calls the card shows: `1 of 3`. */
export const place = (index: number, count: number) => `${index} of ${count}`;
/** On the card when the answer sent from its notification did not go. */
export const pushFailed = (why: string) => `Your answer from the notification didn't go: ${why}`;

/** How much a hidden answer holds, so the person can see their typing and paste landed. */
export function hidden(text: string): string {
    const lines = (text.endsWith("\n") ? text.slice(0, -1) : text).split("\n").length;
    const characters = `${text.length} character${text.length === 1 ? "" : "s"} hidden`;
    return lines > 1 ? `${lines} lines, ${characters}` : characters;
}

/** The name on a picture's corner: an evidence item's own words, when they are short enough to be a name, else none. */
export function pictureLabel(words: string): string | undefined {
    const trimmed = words.trim();
    const count = trimmed.split(/\s+/).filter(Boolean).length;
    return trimmed && trimmed.length <= 20 && count <= 3 ? trimmed : undefined;
}

/** Said when an answer to a call carries context chips, which a call's answer does not take. */
export const NO_CHIPS = "A call's answer takes words and files, not chips. Take the chips off, or send them to the agent.";
/** The composer's chip while a call waits in the chat. */
export const answering = (name: string, question: string) => `Answering @${name} · ${question}`;
/** The composer's placeholder while a call waits in the chat. */
export const typeYourAnswer = (name: string) => `Or type your answer to @${name}`;

/** A weighty call's brief, which stands as its context. */
export function context(call: PersonCall): string | undefined {
    return call.brief ? `${call.brief.tried} ${call.brief.shows} ${call.brief.cannotUndo}` : (call.context ?? undefined);
}

// The calls tab

export const CALLS = "Calls";
export const ANSWERED = "Answered";
export const NO_CALLS = "Nothing waits on you.";

/** What came of an answered or closed call, as its row in the calls tab says it. */
export function cameOf(call: PersonCall): string {
    switch (call.outcome?.result) {
        case "shipped":
            return call.outcome.healthy === true ? "shipped · healthy" : call.outcome.healthy === false ? "shipped · unhealthy" : "shipped";
        case "notShipped":
            return "not shipped";
        case "failed":
            return "it failed";
        case "mayHaveRun":
            return "may have run, Sikemux restarted";
        case "handedBack":
            return "handed back";
        case "withdrawn":
            return "went on without you";
    }
    const answer = call.answer;
    if (!answer) return "closed";
    if (call.secret) return "answered, kept private";
    if (answer.words != null) return "in your words";
    const picked = answer.option != null ? [answer.option] : (answer.options ?? []);
    if (picked.length === 1 && picked[0] === call.pick) return "as picked";
    const labels = picked.map((id) => call.options.find((option) => option.id === id)?.label).filter((label) => label != null);
    return labels.length ? labels.map((label) => `“${label}”`).join(", ") : "answered";
}

// The push

/** The push's button that opens a field for the person's own answer, and the field's button. */
export const SOMETHING_ELSE = "Something else…";
export const REPLY = "Reply";
