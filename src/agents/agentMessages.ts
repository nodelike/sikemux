/** What a message between agents was: a message, a worker's report to its lead, a question, its answer, or a note from Sikemux itself. */
export type MessageKind = "message" | "report" | "question" | "answer" | "notice";

/** What became of a message, as the core logs it. */
export type MessageOutcome = "steered" | "newTurn" | "typed" | "waiting" | "delivered" | "returned" | "repeat" | "refused" | "dropped";

/** Held waits for its agent; failed never reached it. */
export type MessageTone = "held" | "failed" | null;

/** One message between agents, from being sent to what last became of it, as the core folds its log. Never its text. */
export interface MessageRow {
    sentAt: number;
    at: number;
    /** The sender's agent id, or `person`, `board`, `auto` or `sikemux`; none for one that came back unread. */
    from: string | null;
    /** The sending agent's name now, else when it was logged; none for senders that are not agents. */
    fromName: string | null;
    to: string;
    toName: string | null;
    kind: MessageKind | null;
    outcome: MessageOutcome;
    /** It waited for its agent before it got where it is. */
    waited: boolean;
    tone: MessageTone;
}

/** What the core's `agentMessageRows` call answers: the project's own path and its messages, newest first. */
export interface AgentMessageRows {
    project: string;
    rows: MessageRow[];
}

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

export function outcomeLabel(row: Pick<MessageRow, "outcome" | "waited">): string {
    return row.outcome === "delivered" && row.waited ? "Delivered after waiting" : OUTCOMES[row.outcome];
}

export function kindLabel(kind: MessageKind | null): string {
    return kind ? KINDS[kind] : "Message";
}

/** Who sent or got a message: a sender that is not an agent by what it is, an agent by its name. */
export function whoLabel(id: string | null, name: string | null): string {
    return id ? (SENDERS[id] ?? name ?? "A closed agent") : "An agent";
}

/** Each row's key among `rows`, from when it was sent, who sent it and to whom. */
export function rowKeys(rows: readonly MessageRow[]): string[] {
    const seen = new Map<string, number>();
    return rows.map((row) => {
        const key = `${row.sentAt}:${row.from ?? ""}:${row.to}`;
        const repeats = seen.get(key) ?? 0;
        seen.set(key, repeats + 1);
        return repeats ? `${key}:${repeats}` : key;
    });
}
