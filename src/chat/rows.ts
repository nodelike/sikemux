import type { ChatChange, ChatHead, ChatMessage, ChatPage, ChatPart, ChatView, PartTree } from "../api/generated/wire";
import { applyPatch } from "../markdown/patch";
import type { MdChange, MdElement } from "../markdown/types";

/** A chat as a client holds it: how it stands, its rows, and each subagent's own rows. */
export interface ChatRows extends Omit<ChatHead, "capabilities" | "setup"> {
    capabilities: Record<string, unknown>;
    setup: Record<string, unknown>;
    messages: ChatMessage[];
    subagentRows: Record<string, ChatMessage[]>;
    /** Earlier rows start before this event; the core pages back to them. */
    olderBefore: number | null;
}

export const emptyChat: ChatRows = {
    connection: "connecting",
    running: false,
    turned: false,
    permissionMode: "",
    commands: [],
    capabilities: {},
    setup: {},
    permissions: [],
    tasks: [],
    runningSubagents: [],
    queued: [],
    earlierLost: false,
    messages: [],
    subagentRows: {},
    olderBefore: null,
};

export function fromView(view: ChatView): ChatRows {
    return {
        ...emptyChat,
        ...(view.head as Omit<ChatHead, "capabilities" | "setup">),
        capabilities: (view.head.capabilities ?? {}) as Record<string, unknown>,
        setup: (view.head.setup ?? {}) as Record<string, unknown>,
        messages: view.rows,
        subagentRows: view.subagents ?? {},
        olderBefore: view.olderBefore ?? null,
    };
}

/** `fields` laid over `on`; a field the core sends as null is gone. */
function merged<T extends object>(on: T, fields: unknown): T {
    const next = { ...on } as Record<string, unknown>;
    for (const [key, value] of Object.entries(fields as Record<string, unknown>)) {
        if (value === null) delete next[key];
        else next[key] = value;
    }
    return next as unknown as T;
}

/* A part's tree follows the core's changes while each one fits the text it
   ends at; one that does not loses it until the core sends a whole tree. */
function nextTree(tree: PartTree | undefined, change: MdChange | undefined, text: string): PartTree | undefined {
    if (!change) return tree;
    const first = change.o[0];
    const whole = first !== undefined && first.p.length === 0 && first.at === 0 && "c" in first;
    if (!whole && !tree) return undefined;
    const blocks = applyPatch(whole ? [] : ((tree?.blocks ?? []) as MdElement[]), change.o);
    return change.n === text.length ? { blocks, n: change.n } : undefined;
}

function withRow(rows: ChatMessage[], id: string, change: (row: ChatMessage) => ChatMessage): ChatMessage[] {
    for (let at = rows.length - 1; at >= 0; at -= 1) {
        if (rows[at].id !== id) continue;
        const next = [...rows];
        next[at] = change(rows[at]);
        return next;
    }
    return rows;
}

function withPart(row: ChatMessage, id: string, change: (part: ChatPart) => ChatPart): ChatMessage {
    const at = row.parts.findIndex((part) => part.id === id);
    if (at < 0) return row;
    const parts = [...row.parts];
    parts[at] = change(row.parts[at]);
    return { ...row, parts };
}

function putPart(row: ChatMessage, part: ChatPart): ChatMessage {
    const at = row.parts.findIndex((held) => held.id === part.id);
    if (at < 0) return { ...row, parts: [...row.parts, part] };
    const parts = [...row.parts];
    parts[at] = part;
    return { ...row, parts };
}

function putRow(rows: ChatMessage[], row: ChatMessage): ChatMessage[] {
    for (let at = rows.length - 1; at >= 0; at -= 1) {
        if (rows[at].id !== row.id) continue;
        const next = [...rows];
        next[at] = row;
        return next;
    }
    return [...rows, row];
}

function keep(rows: ChatMessage[], ids: readonly string[]): ChatMessage[] {
    const kept = new Set(ids);
    const first = rows.findIndex((row) => kept.has(row.id));
    if (first < 0) return [];
    return [...rows.slice(0, first), ...rows.slice(first).filter((row) => kept.has(row.id))];
}

function within(state: ChatRows, session: string | undefined, change: (rows: ChatMessage[]) => ChatMessage[]): ChatRows {
    if (session === undefined) return { ...state, messages: change(state.messages) };
    const rows = state.subagentRows[session] ?? [];
    return { ...state, subagentRows: { ...state.subagentRows, [session]: change(rows) } };
}

function applyChange(state: ChatRows, change: ChatChange): ChatRows {
    switch (change.op) {
        case "head":
            return merged(state, change.set);
        case "row":
            return within(state, change.in, (rows) => putRow(rows, change.row));
        case "part":
            return within(state, change.in, (rows) => withRow(rows, change.row, (row) => putPart(row, change.part)));
        case "text":
            return within(state, change.in, (rows) =>
                withRow(rows, change.row, (row) =>
                    withPart(row, change.part, (part) => {
                        if (part.kind !== "text" && part.kind !== "thought") return part;
                        const text = part.text + change.text;
                        return { ...part, text, md: nextTree(part.md, change.md as MdChange | undefined, text) };
                    }),
                ),
            );
        case "set":
            return within(state, change.in, (rows) => withRow(rows, change.row, (row) => merged(row, change.set)));
        case "keep":
            return { ...state, messages: keep(state.messages, change.rows) };
        case "drop": {
            const at = state.messages.findIndex((row) => row.id === change.from);
            return at < 0 ? state : { ...state, messages: state.messages.slice(0, at) };
        }
        case "view":
            return fromView(change.view);
    }
}

/** The chat with the core's changes made, keeping every row and part they leave alone as the object it was. */
export function applyChanges(state: ChatRows, changes: readonly ChatChange[]): ChatRows {
    return changes.reduce(applyChange, state);
}

/** Rows from before the first one held go above it. */
export function withPage(state: ChatRows, page: ChatPage): ChatRows {
    const held = new Set(state.messages.map((row) => row.id));
    const subagentRows = { ...(page.subagents ?? {}), ...state.subagentRows };
    return {
        ...state,
        messages: [...page.rows.filter((row) => !held.has(row.id)), ...state.messages],
        subagentRows,
        olderBefore: page.olderBefore ?? null,
    };
}
