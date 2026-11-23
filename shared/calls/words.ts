import type { PersonCall, TurnPhase, TurnState, TurnStep, TurnWait, WorkAt } from "../wire.generated";
import { clockLabel } from "../chat/messageMeta";
import { elapsedLabel } from "../chat/durationLabel";
import { formatAge } from "../lib/format";
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
    switch (turn.state) {
        case "working":
            if (!turn.on) return undefined;
            return { word: say("Working"), phase: "working", parts: [lowered(turn.on), elapsed(Math.floor((now - turn.since) / 1000), say)] };
        case "waiting": {
            if (!turn.wait) return undefined;
            return { word: waitingOn(turn.wait, say), phase: "waiting", parts: waited(turn.wait, now, say) };
        }
        case "needsYou": {
            const call = turn.call;
            if (!call) return undefined;
            const asked = call.count > 1 ? say("{question} and {more} more", { question: call.question, more: call.count - 1 }) : call.question;
            return { word: say("Needs you"), phase: "needsYou", parts: [asked, formatAge(call.waitingSince, now, say)] };
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

/** The agent a wait names first, with how many more it waits on: `@Otter +14`. */
function agentsWaitedOn(wait: TurnWait): string {
    const first = wait.what.startsWith("@") ? wait.what : `@${wait.what}`;
    const more = (wait.agents?.length ?? 0) - 1;
    return more > 0 ? `${first} +${more}` : first;
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
            return say("Waiting on {what}", { what: agentsWaitedOn(wait) });
        case "timer":
            return wait.until != null ? say("Waiting until {when}", { when: clockLabel(wait.until, undefined, undefined, say) }) : say("Waiting");
        case "line":
            return wait.what;
        case "inUse":
            return say("Waiting for {what}", { what: wait.what });
        case "land":
            return say("Waiting to land on {branch}", { branch: wait.what });
        default:
            return say("Waiting on {what}", { what: wait.what });
    }
}

/**
 * How long a wait has gone, against how long it usually takes once that is known; for a command in line, its place and
 * about how long it has left.
 */
export function waited(wait: TurnWait, now = Date.now(), say: Say = english): string[] {
    if (wait.kind === "timer") return [];
    if (wait.kind === "line" || wait.kind === "land") {
        return [...(wait.place != null ? [inLine(wait.place, say)] : []), ...(wait.expectedMs != null ? [about(wait.expectedMs, say)] : [])];
    }
    const gone = Math.floor(Math.max(0, now - wait.startedAt) / MINUTE);
    if (wait.kind === "inUse") {
        return [
            ...(wait.by != null ? [say("in use by @{name}", { name: wait.by })] : []),
            ...(gone > 0 ? [say("{minutes} min", { minutes: gone })] : []),
        ];
    }
    if (wait.expectedMs == null) return [say("{minutes} min", { minutes: gone })];
    return [say("{minutes} of ~{expected} min", { minutes: gone, expected: Math.ceil(wait.expectedMs / MINUTE) })];
}

/** A place in the host's line on its own: "next in line", "2nd in line". */
export function inLine(at: number, say: Say = english): string {
    return at > 1 ? say("{place} in line", { place: ordinal(at, say) }) : say("next in line");
}

function ordinal(at: number, say: Say): string {
    if ([11, 12, 13].includes(at % 100)) return say("{place}th", { place: at });
    switch (at % 10) {
        case 1:
            return say("{place}st", { place: at });
        case 2:
            return say("{place}nd", { place: at });
        case 3:
            return say("{place}rd", { place: at });
        default:
            return say("{place}th", { place: at });
    }
}

/** How long a wait has left, roughly: "about 4 min". */
export function about(ms: number, say: Say = english): string {
    return say("about {minutes} min", { minutes: Math.max(1, Math.ceil(ms / MINUTE)) });
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
        return turn.call ? formatAge(turn.call.waitingSince, now, say) : undefined;
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

/** The chief's row: @Chief, its role, and where projects share the list, the project. */
export const CHIEF = "@Chief";
export const CHIEF_ROLE = "Chief of staff";
/** The composer's words in the chief's own chat. */
export const TELL_CHIEF = "Tell @Chief what you want";
/** Under the chief's row when a tap cannot reach the host to open its chat. */
export const CHIEF_OFFLINE = "Not connected to this host yet, so its chief can't open.";
/** On the chief's row while the host it lives on is out of reach. */
export const TAKE_OVER = "Take over here";
/** Under the chief's row while the host it lives on is out of reach, since `at`. */
export const chiefAway = (at: number, now = Date.now(), say: Say = english) =>
    say("Out of reach since {time}", { time: clockLabel(at, now, undefined, say) });
/** The composer's words in a worker's chat whose project's chief hears what is typed there. */
export const sendToTold = (name: string, say: Say = english) => say("Send to @{name} · @Chief is told", { name });

// The chief's chat: an Overrule, the week's count

export const OVERRULE = "Overrule";
/** Under an Overrule or Undo that has no host to send it to. */
export const NOT_CONNECTED = "Not connected to the host.";
export const UNDO = "Undo";
/** The line under an agent's reply that says it recorded a decision itself. */
export const DECIDED = "Decided";
export const OVERRULED = "Overruled";
export const OVERRULED_UNDONE = "Overruled, then undone";
export const STANDS_AGAIN = "It stands again";
/** Undoing an Overrule could not remove the rule it kept. */
export const STILL_KEPT = "It stands again, but decisions like this still come to you: remove the rule in the Playbook";
export const FROM_NOW_ON = "Decisions like this come to you from now on";
/** The line the board leaves when it closed a card by itself. */
export const CLOSED_BY_ITSELF = "Closed by itself";
export const CLOSED_REOPENED = "Closed by itself, then reopened";
export const checkPassed = (commit: string, say: Say = english) => say("The check passed on {commit}", { commit: commit.slice(0, 7) });
export const RULE_BACK = "Rule back";
export const NOW_A_CHECK = "Now a check";
export const RULE_CHANGED = "Rule changed";
export const RULE_REMOVED = "Rule removed";
/** Who changed a rule, after it on its line. */
export const byName = (name: string, say: Say = english) => say("by @{name}", { name });
/** A rule change's line as the Mac words it: its title, then the short hash of the check that holds it or who did it. */
export function ruleChange(line: { restored?: boolean; commit?: string; was?: string; by: string; backBy?: string }, say: Say = english) {
    if (line.restored) return { head: say(RULE_BACK), tail: line.backBy ? byName(line.backBy, say) : undefined };
    if (line.commit) return { head: say(NOW_A_CHECK), tail: line.commit.slice(0, 7) };
    return { head: say(line.was ? RULE_CHANGED : RULE_REMOVED), tail: byName(line.by, say) };
}
/** What was decided, quoted, never as the person's own words. */
export const quoted = (text: string, say: Say = english) => say("“{text}”", { text });
/** When the week's count came: `Monday · 09:00`, or `Monday · 9:00 AM` on a 12-hour clock. */
export function weekCame(at: number, say: Say = english, hour12 = false): string {
    const days = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
    const day = say(days[new Date(at).getDay()]);
    const time = new Date(at).toLocaleTimeString(undefined, { hour: hour12 ? "numeric" : "2-digit", minute: "2-digit", hour12 });
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

// The call card

export const PICK = "pick";
export const AGAINST_IT = "Against it:";
export const ADD_NOTE = "Add a note…";
export const SECRET_FIELD = "Type or paste it…";
export const GO_WITH_THIS = "Go with this";
export const goWith = (option: string, say: Say = english) => say("Go with {option}", { option });
export const OPEN_CHAT = "Open chat";
export const NEXT_CALL = "Next call";
export const WHY_IT_CAME_UP = "Why it came up";
export const WHAT_RUNS = "What runs";
export const kept = (name: string, say: Say = english) => say("Kept private: @{name} reads it, and it never shows in the chat.", { name });
/** Go with this while the host has not yet taken that this phone drew the call. */
export const CONNECTING = "Connecting…";
/** The answers a do or look call takes, by the ids the core gives its options. */
export const DO_DONE = "done";
export const DO_NOT_DOING = "notDoing";
export const LOOK_SEEN = "seen";
/** A do call's Go before an answer is chosen, and a look call's. */
export const DONE = "Done";
export const SEEN = "Seen";
/** The field under Not doing on a do call. */
export const WHY_NOT_DOING = "Say why you're not doing it…";
/** A call's page title: its short title, else who asked it. */
export function pageTitle(call: PersonCall): string {
    return call.title?.trim() || `@${call.asker.name}`;
}

/** What the composer under a call's page says while it is empty. */
export function composerHint(call: PersonCall, chosen: boolean, say: Say = english): string {
    if (call.secret) return say(chosen ? ADD_NOTE : SECRET_FIELD);
    if (!call.options.length) return say("Answer in your own words…");
    if (!chosen) return say("Pick one above, or answer in your own words…");
    return say(call.kind === "production" ? "Add a note, then tap Go…" : ADD_NOTE);
}

/** In a chat, the calls its agent waits on folded into one row. */
export const waitingCalls = (count: number, say: Say = english) => say("{count} calls", { count });
export const newest = (question: string, say: Say = english) => say("Newest: {question}", { question });

/** A do or look call goes by its kind and short title, `Do · Test purchase`; any other by its question. */
export function headline(call: PersonCall, say: Say = english): string {
    const title = call.title?.trim();
    if (!title) return call.question;
    if (call.kind === "do") return say("Do · {title}", { title });
    if (call.kind === "look") return say("Look · {title}", { title });
    return call.question;
}

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
        case "withdrawn":
            return say("went on without you");
    }
    const answer = call.answer;
    if (!answer) return say("closed");
    const picked = answer.option != null ? [answer.option] : (answer.options ?? []);
    if (call.secret && !picked.length) return say("answered, kept private");
    if (answer.words != null) return say("in your words");
    if (picked.length === 1 && picked[0] === call.pick) return say("as picked");
    const labels = picked.map((id) => call.options.find((option) => option.id === id)?.label).filter((label) => label != null);
    return labels.length ? labels.map((label) => say("“{label}”", { label })).join(", ") : say("answered");
}

// The push

/** The push's button that opens a field for the person's own answer, and the field's button. */
export const SOMETHING_ELSE = "Something else…";
export const REPLY = "Reply";
