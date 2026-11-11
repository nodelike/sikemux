import { durationLabel } from "./durationLabel";
import { toolRunning } from "./toolLabels";
import type { ChatPart, RunClause, RunLine, ToolFamily, ToolIntent, ToolRow, ToolRun } from "../wire.generated";

type ToolPart = Extract<ChatPart, { kind: "tool" }>;

/** The most things a merged line names before it counts the rest. */
export const LINE_NAMES = 4;
/** The most clauses a strip's sentence says before it counts the rest. */
const SENTENCE_CLAUSES = 4;

const intentOf = (row: ToolRow): ToolIntent => row.intent ?? "use";
const familyOf = (row: ToolRow): ToolFamily => row.family ?? "other";
const callsOf = (row: ToolRow): number => row.times ?? 1;

export const callFailed = (part: ToolPart): boolean => part.tool.status === "failed" || part.failure !== undefined;

export const callTook = (part: ToolPart): number =>
    part.startedAt !== undefined && part.endedAt !== undefined ? Math.max(0, part.endedAt - part.startedAt) : 0;

/** The command a call ran, as it was typed. */
export const callCommand = (part: ToolPart): string | null => (part.tool.kind === "execute" ? part.tool.title.trim() : null);

/** Whether a call has more to show: a change, what it printed, why it failed, or a command longer than its line. */
export function callOpens(part: ToolPart): boolean {
    const command = callCommand(part);
    const cutOff = command !== null && (toolRunning(part.tool) || command !== part.tool.row.target);
    return Boolean(part.diff || part.output || part.failure) || cutOff;
}

/** The run the core counted on its first call, or, when only part of it is in view, one call a line. */
export function runOf(parts: readonly ToolPart[]): ToolRun {
    const counted = parts[0]?.run;
    if (counted) return counted;
    const said: RunClause[] = [];
    for (const part of parts) {
        const row = part.tool.row;
        const clause = said.find((clause) => clause.intent === intentOf(row) && clause.family === familyOf(row));
        if (clause) clause.count += callsOf(row);
        else said.push({ intent: intentOf(row), family: familyOf(row), count: callsOf(row) });
    }
    return {
        said,
        failed: parts.filter(callFailed).length,
        lines: parts.map((part, at) => ({
            intent: intentOf(part.tool.row),
            family: familyOf(part.tool.row),
            calls: [at],
            failed: callFailed(part),
        })),
    };
}

/** What a line names a call by: the file or pattern it touched, else its target. */
export function callName(part: ToolPart): string {
    const row = part.tool.row;
    if (row.subject !== undefined) {
        const agent = row.family === "agents" || row.family === "waiting";
        return agent && !row.subject.startsWith("@") ? `@${row.subject}` : row.subject;
    }
    return row.target;
}

export interface LineName {
    name: string;
    /** The file it names, with the lines it touched after a colon when the call said. */
    path?: string;
}

/** The distinct names of a line's calls, at most `LINE_NAMES`, and how many more there are. */
export function lineNames(calls: readonly ToolPart[]): { names: LineName[]; more: number } {
    const names: LineName[] = [];
    for (const call of calls) {
        const name = callName(call);
        if (name && !names.some((held) => held.name === name)) names.push({ name, path: call.tool.row.path });
    }
    return { names: names.slice(0, LINE_NAMES), more: Math.max(0, names.length - LINE_NAMES) };
}

const LINES = /:(\d+-\d+)$/;

/** `path` without the `:first-last` span a read asked for, and that span set with an en dash; a bare start line is dropped. */
export function placeOf(path: string): { file: string; lines?: string } {
    const match = LINES.exec(path);
    if (match) return { file: path.slice(0, match.index), lines: match[1].replace("-", "–") };
    return { file: path.replace(/:\d+$/, "") };
}

/** A merged line's verb: the clause's own, or "Waited for" before the agents it names. */
export function lineVerb(line: RunLine, named: boolean): string {
    if (line.intent === "wait" && named) return "Waited for";
    return words({ intent: line.intent, family: line.family, count: line.calls.length })[0];
}

const counted = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

function often(n: number): string {
    if (n <= 1) return "";
    return n === 2 ? "twice" : `${n} times`;
}

function withOften(what: string, n: number): string {
    const times = often(n);
    if (!times) return what;
    return what ? `${what} ${times}` : times;
}

function namesOr(clause: RunClause, one: string, many: string): string {
    const names = (clause.names ?? []).map((name) => (name.startsWith("@") ? name : `@${name}`));
    if (names.length === 0) return counted(clause.count, one, many);
    const extra = Math.max(0, clause.count - names.length);
    if (names.length === 1 && extra === 0) return names[0];
    if (names.length === 2 && extra === 0) return `${names[0]} and ${names[1]}`;
    if (clause.count > names.length && names.length >= 3) return counted(clause.count, one, many);
    return names.join(", ");
}

function service(clause: RunClause): string {
    const name = clause.names?.[0] ?? "";
    return name.charAt(0).toUpperCase() + name.slice(1);
}

/** A clause as the phone words it: its verb done and doing, and what it was done to. */
export function words(clause: RunClause): [past: string, now: string, rest: string] {
    const n = clause.count;
    const { intent, family } = clause;
    switch (intent) {
        case "read":
            if (family === "browser") return ["Read", "Reading", withOften("the page", n)];
            if (family === "simulator") return ["Read", "Reading", withOften("the app", n)];
            if (family === "agents") return ["Read", "Reading", namesOr(clause, "agent", "agents")];
            return ["Read", "Reading", counted(n, "file", "files")];
        case "view":
            return ["Viewed", "Viewing", counted(n, "image", "images")];
        case "search":
            return family === "web" ? ["Searched", "Searching", withOften("the web", n)] : ["Searched", "Searching", often(n)];
        case "list":
            return ["Listed", "Listing", counted(n, "folder", "folders")];
        case "edit":
            return ["Edited", "Editing", counted(n, "file", "files")];
        case "write":
            return ["Wrote", "Writing", counted(n, "file", "files")];
        case "move":
            return ["Moved", "Moving", counted(n, "file", "files")];
        case "delete":
            return ["Deleted", "Deleting", counted(n, "file", "files")];
        case "run":
            return ["Ran", "Running", counted(n, "command", "commands")];
        case "test":
            return ["Ran", "Running", withOften("tests", n)];
        case "build":
            return ["Ran", "Running", counted(n, "build", "builds")];
        case "git":
            return ["Ran", "Running", counted(n, "git command", "git commands")];
        case "commit":
            return ["Committed", "Committing", counted(n, "change", "changes")];
        case "open":
            return family === "browser" ? ["Opened", "Opening", counted(n, "page", "pages")] : ["Opened", "Opening", withOften("the app", n)];
        case "click":
            return family === "simulator" ? ["Tapped", "Tapping", often(n)] : ["Clicked", "Clicking", often(n)];
        case "type":
            return ["Typed", "Typing", often(n)];
        case "look":
            return ["Took", "Taking", counted(n, "screenshot", "screenshots")];
        case "message":
            return ["Messaged", "Messaging", namesOr(clause, "agent", "agents")];
        case "start":
            return family === "agents"
                ? ["Started", "Starting", namesOr(clause, "agent", "agents")]
                : ["Started", "Starting", counted(n, "process", "processes")];
        case "stop":
            return family === "agents"
                ? ["Closed", "Closing", namesOr(clause, "agent", "agents")]
                : ["Stopped", "Stopping", counted(n, "process", "processes")];
        case "wait": {
            const took = clause.took !== undefined && clause.took >= 1000 ? durationLabel(clause.took) : null;
            const whom = clause.names?.length ? namesOr(clause, "agent", "agents") : null;
            const rest = took && whom ? `${took} for ${whom}` : (took ?? (whom ? `for ${whom}` : ""));
            return ["Waited", "Waiting", rest];
        }
        case "query":
            return ["Asked", "Asking", withOften(service(clause), n)];
        case "fetch":
            return ["Read", "Reading", counted(n, "page", "pages")];
        case "mode":
            return ["Switched mode", "Switching mode", ""];
        case "use":
            return clause.names?.length ? ["Used", "Using", withOften(service(clause), n)] : ["Used", "Using", counted(n, "tool", "tools")];
    }
}

const lowerFirst = (text: string) => text.charAt(0).toLowerCase() + text.slice(1);

/**
 * The run's sentence: each thing it did, counted, in the order it first did it. A run that only
 * looked says it explored; a running one leads with what it is doing now.
 */
export function sentence(run: ToolRun, running: ToolRow | undefined): string {
    const doing = running
        ? run.said.findIndex(
              (clause) =>
                  clause.intent === intentOf(running) &&
                  (clause.family === familyOf(running) || (clause.family === "files" && familyOf(running) === "shell")),
          )
        : -1;
    const order = [...(doing >= 0 ? [doing] : []), ...run.said.map((_, at) => at).filter((at) => at !== doing)];
    const clauses = order.slice(0, SENTENCE_CLAUSES).map((at, place) => {
        const [past, now, rest] = words(run.said[at]);
        const said = rest ? `${at === doing ? now : past} ${rest}` : at === doing ? now : past;
        return place === 0 && !run.explored ? said : lowerFirst(said);
    });
    const more = run.said.length - SENTENCE_CLAUSES;
    if (more > 0) clauses.push(`${more} more`);
    const said = clauses.join(", ");
    if (!run.explored) return said;
    return `${doing >= 0 ? "Exploring" : "Explored"}: ${said}`;
}

/** Whether a run's lines already say everything its raw calls would, so "All N calls" adds nothing. */
export function linesSayAll(run: ToolRun, parts: readonly ToolPart[]): boolean {
    return !run.hidden && run.lines.length === parts.length && parts.every((part) => callsOf(part.tool.row) === 1);
}

export const runCalls = (parts: readonly ToolPart[]): number => parts.reduce((sum, part) => sum + callsOf(part.tool.row), 0);
