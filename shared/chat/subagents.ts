import type { ChatMessage, SubagentCard } from "../wire.generated";

/** The chat's subagents: each one's card by its session, for a notice that names one, and the rows it wrote. */
export type Subagents = { card: (sessionId: string) => SubagentCard | undefined; rows: Readonly<Record<string, ChatMessage[]>> };

/** The cards of the chat's subagents as its rows carry them, looked up only when a notice names one. */
export function subagentsOf(messages: readonly ChatMessage[], rows: Readonly<Record<string, ChatMessage[]>>): Subagents {
    const card = (sessionId: string) => {
        for (const held of [messages, ...Object.values(rows)]) {
            for (let at = held.length - 1; at >= 0; at -= 1) {
                for (const part of held[at].parts) if (part.kind === "subagent" && part.subagent.sessionId === sessionId) return part.subagent;
            }
        }
        return undefined;
    };
    return { card, rows };
}
