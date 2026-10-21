/** What a message between agents was: a message, a worker's report to its lead, a question, its answer, or a note from Sikemux itself. */
export type MessageKind = "message" | "report" | "question" | "answer" | "notice";

/** What became of a message, as the core logs it. */
export type MessageOutcome = "steered" | "newTurn" | "typed" | "waiting" | "delivered" | "returned" | "repeat" | "refused" | "dropped";

/** One line of a project's log: a message sent, or what later became of it. Never its text. */
export interface LoggedMessage {
    at: number;
    /** The sender's agent id, `person`, `board`, `auto` or `sikemux`; unknown for one that came back unread. */
    from?: string | null;
    /** What the sender was called then. */
    fromName?: string | null;
    kind?: MessageKind | null;
    to: string;
    toName?: string | null;
    outcome: MessageOutcome;
}

/** What the core's `agentMessages` call answers: the project's own path and its log, newest first. */
export interface AgentMessageLog {
    project: string;
    entries: LoggedMessage[];
}

/** One message between agents, from being sent to what last became of it. */
export interface MessageRow {
    key: string;
    sentAt: number;
    at: number;
    from: string | null;
    fromName: string;
    to: string;
    toName: string;
    kind: MessageKind | null;
    outcome: MessageOutcome;
    /** It waited for its agent before it got where it is. */
    waited: boolean;
}

/** Held waits for its agent; failed never reached it. */
export type MessageTone = "held" | "failed" | null;

const STILL_GOING: ReadonlySet<MessageOutcome> = new Set(["typed", "waiting", "returned"]);
const LATER: ReadonlySet<MessageOutcome> = new Set(["delivered", "returned", "dropped"]);

const SENDERS: Readonly<Record<string, string>> = { person: "You", board: "Agent board", auto: "Auto", sikemux: "Sikemux" };

const OUTCOMES: Readonly<Record<MessageOutcome, string>> = {
    steered: "Read mid-turn",
    newTurn: "Started a turn",
    typed: "Typed in",
    waiting: "Waiting",
    delivered: "Delivered",
    returned: "Not taken, waiting",
    repeat: "Repeat, not sent",
    refused: "Refused",
    dropped: "Dropped unread",
};

const KINDS: Readonly<Record<MessageKind, string>> = {
    message: "Message",
    report: "Report",
    question: "Question",
    answer: "Answer",
    notice: "Note",
};

export function messageTone(outcome: MessageOutcome): MessageTone {
    if (outcome === "waiting" || outcome === "returned") return "held";
    if (outcome === "refused" || outcome === "dropped") return "failed";
    return null;
}

export function outcomeLabel(row: Pick<MessageRow, "outcome" | "waited">): string {
    return row.outcome === "delivered" && row.waited ? "Delivered after waiting" : OUTCOMES[row.outcome];
}

export function kindLabel(kind: MessageKind | null): string {
    return kind ? KINDS[kind] : "Message";
}

/**
 * The log as one row per message, newest first. The core logs a message when it is sent and again when one that
 * waited is delivered, taken back or dropped; a later line belongs to the oldest message still waiting for the same
 * agent from the same sender. `nameOf` gives an open agent's current name, ahead of the name it had when logged.
 */
export function messageRows(entries: readonly LoggedMessage[], nameOf: (agentId: string) => string | undefined): MessageRow[] {
    const rows: MessageRow[] = [];
    const going: MessageRow[] = [];
    const keys = new Map<string, number>();
    const name = (id: string | null | undefined, logged: string | null | undefined) =>
        id ? (SENDERS[id] ?? nameOf(id) ?? logged ?? "A closed agent") : "An agent";
    for (let index = entries.length - 1; index >= 0; index--) {
        const entry = entries[index];
        if (LATER.has(entry.outcome)) {
            const at = going.findIndex(
                (row) => row.to === entry.to && (!entry.from || row.from === entry.from) && (!entry.kind || row.kind === entry.kind),
            );
            if (at >= 0) {
                const row = going[at];
                row.waited ||= row.outcome !== "typed";
                row.outcome = entry.outcome;
                row.at = entry.at;
                if (!STILL_GOING.has(entry.outcome)) going.splice(at, 1);
                continue;
            }
        }
        const key = `${entry.at}:${entry.from ?? ""}:${entry.to}`;
        const repeats = keys.get(key) ?? 0;
        keys.set(key, repeats + 1);
        const row: MessageRow = {
            key: repeats ? `${key}:${repeats}` : key,
            sentAt: entry.at,
            at: entry.at,
            from: entry.from ?? null,
            fromName: name(entry.from, entry.fromName),
            to: entry.to,
            toName: name(entry.to, entry.toName),
            kind: entry.kind ?? null,
            outcome: entry.outcome,
            waited: false,
        };
        rows.push(row);
        if (STILL_GOING.has(entry.outcome)) going.push(row);
    }
    return rows.reverse();
}
