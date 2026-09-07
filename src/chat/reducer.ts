import type { AcpAvailableCommand, AcpContentBlock, AcpContentChunk, AcpToolCall, ChatAction, ChatMessage, ChatPart, ChatState } from "./types";

export const initialChatState: ChatState = {
    connection: "connecting",
    messages: [],
    commands: [],
    permissions: [],
    capabilities: {},
    setup: {},
    plan: null,
    usage: null,
    running: false,
    suppressUserEcho: false,
    error: null,
    title: null,
    stopReason: null,
    nextId: 1,
    revision: 0,
};

const textOf = (value: unknown): string | undefined => (typeof value === "string" ? value : undefined);
const recordOf = (value: unknown): Record<string, unknown> | undefined =>
    typeof value === "object" && value !== null && !Array.isArray(value) ? (value as Record<string, unknown>) : undefined;

function contentChunk(update: Record<string, unknown>): AcpContentChunk | null {
    const content = recordOf(update.content);
    if (!content || typeof content.type !== "string") return null;
    return {
        content: content as AcpContentBlock,
        ...(typeof update.messageId === "string" ? { messageId: update.messageId } : {}),
    };
}

function appendChunk(state: ChatState, role: ChatMessage["role"], partKind: "text" | "thought", chunk: AcpContentChunk): ChatState {
    if (role === "user" && state.suppressUserEcho) return state;
    const contentText = textOf(chunk.content.text);
    const lastMessage = state.messages.at(-1);
    const messageId =
        chunk.messageId ?? (lastMessage?.role === role && !lastMessage.id.startsWith("local-") ? lastMessage.id : `${role}-fallback-${state.nextId}`);
    const existingIndex = state.messages.findIndex((message) => message.id === messageId);
    const messages = [...state.messages];
    let nextId = state.nextId;

    if (existingIndex < 0) {
        const part: ChatPart =
            contentText !== undefined
                ? { id: `${messageId}-${partKind}-0`, kind: partKind, text: contentText }
                : { id: `${messageId}-content-0`, kind: "content", content: chunk.content };
        messages.push({ id: messageId, role, parts: [part] });
        nextId += 1;
    } else {
        const message = messages[existingIndex];
        const parts = [...message.parts];
        const last = parts.at(-1);
        if (contentText !== undefined && last?.kind === partKind) {
            parts[parts.length - 1] = { ...last, text: last.text + contentText };
        } else if (contentText !== undefined) {
            parts.push({ id: `${messageId}-${partKind}-${parts.length}`, kind: partKind, text: contentText });
        } else {
            parts.push({ id: `${messageId}-content-${parts.length}`, kind: "content", content: chunk.content });
        }
        messages[existingIndex] = { ...message, parts };
    }

    return {
        ...state,
        messages,
        nextId,
        suppressUserEcho: role === "assistant" ? false : state.suppressUserEcho,
        revision: state.revision + 1,
    };
}

function upsertTool(state: ChatState, update: AcpToolCall, merge: boolean): ChatState {
    const messages = [...state.messages];
    for (let messageIndex = messages.length - 1; messageIndex >= 0; messageIndex -= 1) {
        const message = messages[messageIndex];
        const partIndex = message.parts.findIndex((part) => part.kind === "tool" && part.tool.toolCallId === update.toolCallId);
        if (partIndex < 0) continue;
        const parts = [...message.parts];
        const current = parts[partIndex];
        if (current.kind !== "tool") continue;
        const patch = Object.fromEntries(Object.entries(update).filter(([key, value]) => value !== undefined && !(key === "title" && value === "")));
        parts[partIndex] = {
            ...current,
            tool: merge ? { ...current.tool, ...patch } : update,
        };
        messages[messageIndex] = { ...message, parts };
        return { ...state, messages, suppressUserEcho: false, revision: state.revision + 1 };
    }

    const messageId = `tool-message-${state.nextId}`;
    messages.push({
        id: messageId,
        role: "assistant",
        parts: [{ id: `${messageId}-tool`, kind: "tool", tool: update }],
    });
    return {
        ...state,
        messages,
        nextId: state.nextId + 1,
        suppressUserEcho: false,
        revision: state.revision + 1,
    };
}

function sessionUpdate(state: ChatState, update: Record<string, unknown>): ChatState {
    switch (update.sessionUpdate) {
        case "user_message_chunk": {
            const chunk = contentChunk(update);
            return chunk ? appendChunk(state, "user", "text", chunk) : state;
        }
        case "agent_message_chunk": {
            const chunk = contentChunk(update);
            return chunk ? appendChunk(state, "assistant", "text", chunk) : state;
        }
        case "agent_thought_chunk": {
            const chunk = contentChunk(update);
            return chunk ? appendChunk(state, "assistant", "thought", chunk) : state;
        }
        case "tool_call": {
            const toolCallId = textOf(update.toolCallId);
            const title = textOf(update.title);
            return toolCallId && title ? upsertTool(state, { ...update, toolCallId, title }, false) : state;
        }
        case "tool_call_update": {
            const toolCallId = textOf(update.toolCallId);
            return toolCallId ? upsertTool(state, { ...update, toolCallId, title: textOf(update.title) ?? "" }, true) : state;
        }
        case "plan":
            return { ...state, plan: update, suppressUserEcho: false, revision: state.revision + 1 };
        case "available_commands_update":
            return {
                ...state,
                commands: Array.isArray(update.availableCommands)
                    ? update.availableCommands
                          .filter(
                              (command): command is AcpAvailableCommand =>
                                  typeof command === "object" &&
                                  command !== null &&
                                  typeof (command as AcpAvailableCommand).name === "string" &&
                                  typeof (command as AcpAvailableCommand).description === "string",
                          )
                          .slice(0, 256)
                    : [],
                revision: state.revision + 1,
            };
        case "config_option_update":
            return { ...state, setup: { ...state.setup, configOptions: update.configOptions }, revision: state.revision + 1 };
        case "usage_update":
            return { ...state, usage: update, revision: state.revision + 1 };
        case "session_info_update":
            return { ...state, title: typeof update.title === "string" ? update.title : state.title, revision: state.revision + 1 };
        default:
            return state;
    }
}

export function chatReducer(state: ChatState, action: ChatAction): ChatState {
    switch (action.type) {
        case "config":
            return { ...state, setup: { ...state.setup, configOptions: action.options }, revision: state.revision + 1 };
        case "reset":
            return initialChatState;
        case "status":
            return {
                ...state,
                connection: action.state,
                running: action.state === "stopped" || action.state === "error" ? false : state.running,
                permissions: action.state === "stopped" || action.state === "error" ? [] : state.permissions,
                error: action.state === "error" ? state.error : null,
            };
        case "ready":
            return { ...state, connection: "ready", capabilities: action.capabilities, setup: action.setup, error: null };
        case "local_prompt": {
            const id = `local-${state.nextId}`;
            return {
                ...state,
                messages: [
                    ...state.messages,
                    {
                        id,
                        role: "user",
                        parts: action.text.trim() ? [{ id: `${id}-text`, kind: "text", text: action.text }] : [],
                        ...(action.paths.length ? { attachments: action.paths } : {}),
                    },
                ],
                nextId: state.nextId + 1,
                suppressUserEcho: true,
                running: true,
                error: null,
                revision: state.revision + 1,
            };
        }
        case "session_update":
            return sessionUpdate(state, action.update);
        case "turn_started":
            return { ...state, running: true, stopReason: null, error: null, revision: state.revision + 1 };
        case "turn_completed":
            return {
                ...state,
                running: false,
                suppressUserEcho: false,
                permissions: [],
                stopReason: action.stopReason ?? null,
                revision: state.revision + 1,
            };
        case "permission_requested":
            return {
                ...state,
                permissions: [...state.permissions.filter((request) => request.requestId !== action.request.requestId), action.request],
                revision: state.revision + 1,
            };
        case "permission_cleared":
            return {
                ...state,
                permissions: state.permissions.filter((request) => request.requestId !== action.requestId),
                revision: state.revision + 1,
            };
        case "error":
            return {
                ...state,
                connection: state.connection === "ready" ? "ready" : "error",
                running: false,
                permissions: [],
                error: action.message,
                revision: state.revision + 1,
            };
    }
}
