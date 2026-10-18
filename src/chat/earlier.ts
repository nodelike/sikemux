import { chatReducer, initialChatState } from "./reducer";
import type { ChatAction, ChatMessage, ChatState } from "./types";

/** How many turns one trip back through a long chat brings from the host. */
export const PAGE_TURNS = 20;

/** Said above a chat whose host no longer keeps its first turns. */
export const EARLIER_LOST = "Earlier messages are no longer kept.";

/** A page of turns from before what a remote view holds, as the host sent it. */
export type EarlierPage = { number: number; actions: ChatAction[] };

/**
 * Turns from before what a remote view holds, drawn on their own. A subagent can run on past the
 * page that started it, so what came after the page, `later`, is played on to it too. The ids are
 * renamed so an earlier message can never take the id of one already on screen, which the list
 * keeps its place by.
 */
export function earlierMessages(page: EarlierPage, later: readonly ChatAction[]): ChatMessage[] {
    const own = page.actions.reduce(chatReducer, initialChatState);
    const after = holdsRunningSubagent(own.messages) ? later.reduce(chatReducer, own) : own;
    const ids = new Set(own.messages.map((message) => message.id));
    return after.messages.filter((message) => ids.has(message.id)).map((message) => ({ ...message, id: `earlier-${page.number}-${message.id}` }));
}

/**
 * Whether `action` can still change a subagent an earlier page started: anything said in a
 * session other than the chat's own, `chatSession`, or of a subagent by its id.
 */
export function reachesSubagent(action: ChatAction, chatSession: string | null): boolean {
    return action.type === "session_update" && (action.sessionId !== chatSession || typeof action.update.subagentSessionId === "string");
}

/** Puts turns from before what a remote view holds above it. */
export function withEarlier(state: ChatState, messages: ChatMessage[]): ChatState {
    return chatReducer(state, { type: "earlier", messages });
}

function holdsRunningSubagent(messages: ChatMessage[]): boolean {
    return messages.some((message) =>
        message.parts.some((part) => part.kind === "subagent" && (part.subagent.state === "running" || holdsRunningSubagent(part.subagent.messages))),
    );
}
