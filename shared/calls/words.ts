import type { PersonCall, TurnPhase, TurnState, TurnStep, TurnWait, WorkAt } from "../wire.generated";
import { clockLabel } from "../chat/messageMeta";
import { elapsedLabel } from "../chat/durationLabel";
import { english, type Say } from "../lib/say";

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
export function said(turn: TurnState, maker: string, now = Date.now(), say: Say = english): Said | undefined {
    const minutes = (from: number) => Math.max(1, Math.floor((now - from) / MINUTE));
    switch (turn.state) {
        case "working":
            if (!turn.on) return undefined;
            return { word: say("Working"), phase: "working", parts: [lowered(turn.on), elapsed(Math.floor((now - turn.since) / 1000), say)] };
        case "waiting": {
            if (!turn.wait) return undefined;
            const gone = waited(turn.wait, now, say);
            return { word: waitingOn(turn.wait, say), phase: "waiting", parts: gone ? [gone] : [] };
        }
        case "needsYou": {
            const call = turn.call;
            if (!call) return undefined;
            const asked = call.count > 1 ? say("{question} and {more} more", { question: call.question, more: call.count - 1 }) : call.question;
            return { word: say("Needs you"), phase: "needsYou", parts: [asked, say("{minutes} min", { minutes: minutes(call.waitingSince) })] };
        }
        case "done": {
            const done = turn.done;
            if (!done) return { word: say("Done"), phase: "done", parts: [] };
            const parts = [changed(done.changes, say)];
            if (done.changes !== "none") {
                if (done.checks === "pass") parts.push(done.tests != null ? say("{tests} tests pass", { tests: done.tests }) : say("checks pass"));
                else if (done.checks === "failed") parts.push(say("checks fail"));
            }
            return { word: say("Done"), phase: "done", parts };
        }
        case "stopped": {
            const parts = turn.stopped?.left ? [lowered(turn.stopped.left)] : [];
            parts.push(say("nothing set to wake it"));
            return { word: say("Stopped"), phase: "stopped", parts };
        }
        case "failed": {
            const failed = turn.failed;
            if (!failed) return undefined;
            const why =
                failed.why === "limit"
                    ? say("{maker}'s limit", { maker })
                    : failed.why === "signIn"
                      ? say("signed out of {maker}", { maker })
                      : failed.why === "network"
                        ? say("lost the connection to {maker}", { maker })
                        : (failed.said ?? say("the turn did not finish"));
            const parts = [why];
            if (failed.resetsAt != null && failed.resetsAt > now)
                parts.push(say("resets {when}", { when: clockLabel(failed.resetsAt, now, undefined, say) }));
            return { word: say("Failed"), phase: "failed", parts };
        }
        default:
            return undefined;
    }
}

/**
 * The line over a call docked on the chat, which already shows the question: the word, how many calls wait when more
 * than one does, and how long.
 */
export function docked(line: Said, waiting: number, say: Say = english): Said {
    if (line.phase !== "needsYou") return line;
    return { ...line, parts: [...(waiting > 1 ? [say("{count} calls", { count: waiting })] : []), ...line.parts.slice(-1)] };
}

export function waitingOn(wait: TurnWait, say: Say = english): string {
    switch (wait.kind) {
        case "ci":
            return say("Waiting on CI");
        case "deploy":
            return say("Waiting on the deploy");
        case "check":
            return say("Waiting on the checks");
        case "agent":
            return say("Waiting on {what}", { what: wait.what.startsWith("@") ? wait.what : `@${wait.what}` });
        case "timer":
            return wait.until != null ? say("Waiting until {when}", { when: clockLabel(wait.until, undefined, undefined, say) }) : say("Waiting");
        default:
            return say("Waiting on {what}", { what: wait.what });
    }
}

/** How long a wait has gone, against how long it usually takes once that is known. */
export function waited(wait: TurnWait, now = Date.now(), say: Say = english): string | undefined {
    if (wait.kind === "timer") return undefined;
    const gone = Math.floor(Math.max(0, now - wait.startedAt) / MINUTE);
    if (wait.expectedMs == null) return say("{minutes} min", { minutes: gone });
    return say("{minutes} of ~{expected} min", { minutes: gone, expected: Math.ceil(wait.expectedMs / MINUTE) });
}

export function changed(changes: WorkAt, say: Say = english): string {
    switch (changes) {
        case "uncommitted":
            return say("uncommitted");
        case "committed":
            return say("committed");
        case "landed":
            return say("landed");
        case "staged":
            return say("on staging");
        case "shipped":
            return say("shipped");
        default:
            return say("no changes");
    }
}

/** The words on a step's button. */
export function stepLabel(step: TurnStep, say: Say = english): string | undefined {
    switch (step) {
        case "commit":
            return say("Commit");
        case "push":
            return say("Push");
        case "deployStaging":
            return say("Deploy to staging");
        case "shipProduction":
            return say("Ship to production…");
        case "rollBack":
            return say("Roll back");
        case "tryIt":
            return say("Try it");
        case "carryOn":
            return say("Carry on");
        case "undo":
            return say("Undo");
        default:
            return undefined;
    }
}

/** The button that brings the call a waiting agent asks into view. */
export const LOOK = "Look";

/** How long a turn has run, clock style as the Mac says it: `45s`, `3m 07s`, `2h 05m`. */
export function elapsed(seconds: number, say: Say = english): string {
    return elapsedLabel(Math.max(0, seconds), say);
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
export function rowMinutes(turn: TurnState, now = Date.now(), say: Say = english): string | undefined {
    if (turn.state === "needsYou") {
        return turn.call ? say("{count}m", { count: Math.max(1, Math.floor((now - turn.call.waitingSince) / MINUTE)) }) : undefined;
    }
    if (turn.state !== "waiting" || !turn.wait) return undefined;
    const wait = turn.wait;
    if (ringShare(wait, now) != null && wait.expectedMs) {
        return say("{count}m", { count: Math.max(1, Math.ceil((wait.startedAt + wait.expectedMs - now) / MINUTE)) });
    }
    return wait.until != null ? countdown(wait.until - now, say) : undefined;
}

/** Time left until something fires: now, 12m, 3h, 4d; minutes rounded up under an hour. */
export function countdown(ms: number, say: Say = english): string {
    if (ms <= 0) return say("now");
    const minutes = Math.ceil(ms / MINUTE);
    if (minutes < 60) return say("{count}m", { count: minutes });
    if (minutes < 24 * 60) return say("{count}h", { count: Math.floor(minutes / 60) });
    return say("{count}d", { count: Math.floor(minutes / (24 * 60)) });
}

/** What a row's mark says to a screen reader, the hover card's words on the Mac. */
export function rowLabel(turn: TurnState, now = Date.now(), say: Say = english): string {
    const time = rowMinutes(turn, now, say) ?? "";
    switch (turn.state) {
        case "working":
            return say("Working");
        case "waiting":
            return turn.wait ? say("{waiting}, {time}", { waiting: waitingOn(turn.wait, say), time }) : say("Waiting");
        case "needsYou":
            return say("Needs you, {time}", { time });
        case "done":
            return say("Done");
        case "stopped":
            return say("Stopped");
        case "failed":
            return say("Failed");
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
export const sendTo = (name: string, say: Say = english) => say("Send to @{name}", { name });
/** Under the chief's row when a tap cannot reach the host to open its chat. */
export const CHIEF_OFFLINE = "Not connected to this host yet, so its chief can't open.";
/** The composer's words in a worker's chat whose project's chief hears what is typed there. */
export const sendToTold = (name: string, say: Say = english) => say("Send to @{name} · @Chief is told", { name });

// The chief's chat: what happened while the person was away, an Overrule, the week's count

export const WHILE_AWAY = "While you were away";
/** How long the person was away, `back - since`. */
export function awayFor(ms: number, say: Say = english): string {
    const total = Math.max(1, Math.round(ms / MINUTE));
    const hours = Math.floor(total / 60);
    const minutes = total % 60;
    if (!hours) return say("{minutes} min", { minutes });
    if (!minutes) return say("{hours} h", { hours });
    return say("{hours} h {minutes} min", { hours, minutes });
}
export const AWAY_NEEDS_YOU = "Needs you";
export const DECIDED_FOR_YOU = "Decided for you";
export const LANDED = "Landed";
/** Under Needs you: the calls are the queue docked above the composer. */
export const answerBelow = (count: number, say: Say = english) =>
    count === 1 ? say("Answer it in the card below") : say("Answer them in the card below");
export const OVERRULE = "Overrule";
/** Under an Overrule or Undo that has no host to send it to. */
export const NOT_CONNECTED = "Not connected to the host.";
export const UNDO = "Undo";
export const OVERRULED = "Overruled";
export const OVERRULED_UNDONE = "Overruled, then undone";
export const STANDS_AGAIN = "It stands again";
export const STOPPED_AUTO = "This stopped Auto's run, so it can't be put back";
export const FROM_NOW_ON = "Decisions like this come to you from now on";
/** What was decided, quoted, never as the person's own words. */
export const quoted = (text: string, say: Say = english) => say("“{text}”", { text });
/** When the week's count came: `Monday · 09:00`. */
export function weekCame(at: number, say: Say = english): string {
    const days = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
    const day = say(days[new Date(at).getDay()]);
    const time = new Date(at).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit", hour12: false });
    return say("{day} · {time}", { day, time });
}
/** The chief's count of a week, as the Mac words it. */
export function weekCount(week: { decided: number; overruled: number; answered: number; asPicked: number }, say: Say = english): string {
    const { decided, overruled } = week;
    const first =
        decided === 1
            ? overruled === 0
                ? say("Last week I decided 1 thing, and you overruled none.")
                : say("Last week I decided 1 thing, and you overruled {overruled}.", { overruled })
            : overruled === 0
              ? say("Last week I decided {decided} things, and you overruled none.", { decided })
              : say("Last week I decided {decided} things, and you overruled {overruled}.", { decided, overruled });
    if (!week.answered) return first;
    const second =
        week.answered === 1
            ? say("You answered {picked} of 1 call as I picked.", { picked: week.asPicked })
            : say("You answered {picked} of {answered} calls as I picked.", { picked: week.asPicked, answered: week.answered });
    return say("{first} {second}", { first, second });
}

// An assumption

/** After the agent's name: what it went on assuming. */
export const assumed = (assumption: string, say: Say = english) => say("assumed {assumption}", { assumption });
export const FINE = "Fine";
export const CHANGE = "Change";
export const couldNotRead = (problem: string, say: Say = english) => say("could not read this agent's questions: {problem}", { problem });

// The call card

export const PICK = "pick";
export const AGAINST_IT = "Against it:";
export const ADD_NOTE = "Add a note…";
export const OWN_WORDS = "Or answer in your own words…";
export const SECRET_FIELD = "Type or paste it…";
export const GO_WITH_THIS = "Go with this";
export const NOT_MINE = "Not mine";
export const WHAT_RUNS = "What runs";
/** What the asker goes on with until the person answers, above Go with this. */
export const goesOn = (name: string, assumption: string, say: Say = english) =>
    say("Until you answer, @{name} goes on assuming {assumption}", { name, assumption });
export const kept = (name: string, say: Say = english) => say("Kept private: @{name} reads it, and it never shows in the chat.", { name });
/** Go with this while the host has not yet taken that this phone drew the call. */
export const CONNECTING = "Connecting…";
/** Which of an agent's waiting calls the card shows: `1 of 3`. */
export const place = (index: number, count: number, say: Say = english) => say("{index} of {count}", { index, count });
/** On the card when the answer sent from its notification did not go. */
export const pushFailed = (why: string, say: Say = english) => say("Your answer from the notification didn't go: {why}", { why });

/** How much a hidden answer holds, so the person can see their typing and paste landed. */
export function hidden(text: string, say: Say = english): string {
    const lines = (text.endsWith("\n") ? text.slice(0, -1) : text).split("\n").length;
    const characters = text.length === 1 ? say("1 character hidden") : say("{count} characters hidden", { count: text.length });
    return lines > 1 ? say("{lines} lines, {characters}", { lines, characters }) : characters;
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
export const answering = (name: string, question: string, say: Say = english) => say("Answering @{name} · {question}", { name, question });
/** The composer's placeholder while a call waits in the chat. */
export const typeYourAnswer = (name: string, say: Say = english) => say("Or type your answer to @{name}", { name });

/** A weighty call's brief, which stands as its context. */
export function context(call: PersonCall): string | undefined {
    return call.brief ? `${call.brief.tried} ${call.brief.shows} ${call.brief.cannotUndo}` : (call.context ?? undefined);
}

// The calls tab

export const CALLS = "Calls";
export const ANSWERED = "Answered";
export const NO_CALLS = "Nothing waits on you.";

/** What came of an answered or closed call, as its row in the calls tab says it. */
export function cameOf(call: PersonCall, say: Say = english): string {
    switch (call.outcome?.result) {
        case "shipped":
            return say(call.outcome.healthy === true ? "shipped · healthy" : call.outcome.healthy === false ? "shipped · unhealthy" : "shipped");
        case "notShipped":
            return say("not shipped");
        case "failed":
            return say("it failed");
        case "mayHaveRun":
            return say("may have run, Sikemux restarted");
        case "handedBack":
            return say("handed back");
        case "withdrawn":
            return say("went on without you");
    }
    const answer = call.answer;
    if (!answer) return say("closed");
    if (call.secret) return say("answered, kept private");
    if (answer.words != null) return say("in your words");
    const picked = answer.option != null ? [answer.option] : (answer.options ?? []);
    if (picked.length === 1 && picked[0] === call.pick) return say("as picked");
    const labels = picked.map((id) => call.options.find((option) => option.id === id)?.label).filter((label) => label != null);
    return labels.length ? labels.map((label) => say("“{label}”", { label })).join(", ") : say("answered");
}

// The push

/** The push's button that opens a field for the person's own answer, and the field's button. */
export const SOMETHING_ELSE = "Something else…";
export const REPLY = "Reply";
